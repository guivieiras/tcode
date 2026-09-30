package com.tryanks.tcode;

import android.app.Activity;
import android.app.Dialog;
import android.content.Context;
import android.media.MediaPlayer;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.view.ContextThemeWrapper;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.widget.*;
import org.json.JSONObject;

/** The dialog owns playback and persistent controls; Rust owns the byte stream. */
final class VideoPreview {
    final long id;
    private final Dialog dialog;
    private final VideoView video;
    private final Button play;
    private final Button restart;
    private final Button mute;
    private final SeekBar seek;
    private final TextView time;
    private final JSONObject labels;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private MediaPlayer player;
    private MediaPlayer audioPlayer;
    private HttpMediaSource audioSource;
    private boolean startWhenReady = true;
    private boolean muted;
    private boolean dragging;
    private static native void nativeClosed(long id);

    private final Runnable tick = new Runnable() {
        @Override public void run() {
            updateControls();
            handler.postDelayed(this, 250);
        }
    };

    VideoPreview(Activity activity, long id, String url, String title, String error, String close, boolean dark, JSONObject labels) {
        this.id = id;
        this.labels = labels;
        Context context = new ContextThemeWrapper(activity, dark
                ? android.R.style.Theme_Material_NoActionBar
                : android.R.style.Theme_Material_Light_NoActionBar);
        dialog = new Dialog(context) {
            @Override protected void onStop() {
                // VideoView releases its player when the surface detaches. Stop
                // polling synchronously; OnDismiss runs later on the UI queue.
                releasePlayback();
                super.onStop();
            }
        };
        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
        LinearLayout content = new LinearLayout(context);
        content.setOrientation(LinearLayout.VERTICAL);
        int padding = Math.round(16 * activity.getResources().getDisplayMetrics().density);
        content.setPadding(padding, padding, padding, padding);
        LinearLayout header = new LinearLayout(context);
        header.setGravity(Gravity.CENTER_VERTICAL);
        TextView heading = new TextView(context);
        heading.setText(title);
        heading.setMaxLines(2);
        header.addView(heading, new LinearLayout.LayoutParams(0, -2, 1));
        Button dismiss = new Button(context);
        dismiss.setText(close);
        dismiss.setOnClickListener(v -> dialog.dismiss());
        header.addView(dismiss);
        content.addView(header);
        FrameLayout body = new FrameLayout(context);
        video = new VideoView(context);
        boolean audioOnly = !labels.optString("audio").isEmpty();
        if (!audioOnly) body.addView(video, new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER));
        TextView audio = new TextView(context);
        audio.setText(labels.optString("audio"));
        audio.setGravity(Gravity.CENTER);
        body.addView(audio, new FrameLayout.LayoutParams(-1, -1));
        ProgressBar loading = new ProgressBar(context);
        body.addView(loading, new FrameLayout.LayoutParams(-2, -2, Gravity.CENTER));
        TextView failure = new TextView(context);
        failure.setText(error);
        failure.setGravity(Gravity.CENTER);
        failure.setVisibility(View.GONE);
        body.addView(failure, new FrameLayout.LayoutParams(-1, -1));
        content.addView(body, new LinearLayout.LayoutParams(-1, 0, 1));

        LinearLayout timeline = new LinearLayout(context);
        timeline.setGravity(Gravity.CENTER_VERTICAL);
        seek = new SeekBar(context);
        seek.setMax(1000);
        seek.setContentDescription(labels.optString("seek"));
        timeline.addView(seek, new LinearLayout.LayoutParams(0, -2, 1));
        time = new TextView(context);
        time.setText("0:00 / 0:00");
        timeline.addView(time);
        content.addView(timeline);
        LinearLayout buttons = new LinearLayout(context);
        play = button(context, buttons, "play");
        restart = button(context, buttons, "restart");
        mute = button(context, buttons, "mute");
        content.addView(buttons);
        enableControls(false);
        play.setOnClickListener(v -> {
            if (player.isPlaying()) pausePlayback();
            else {
                if (player.getCurrentPosition() >= player.getDuration()) seekPlayback(0);
                startPlayback();
            }
            updateControls();
        });
        restart.setOnClickListener(v -> { seekPlayback(0); startPlayback(); updateControls(); });
        mute.setOnClickListener(v -> {
            muted = !muted;
            player.setVolume(muted ? 0 : 1, muted ? 0 : 1);
            updateControls();
        });
        seek.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override public void onStartTrackingTouch(SeekBar bar) { dragging = true; }
            @Override public void onStopTrackingTouch(SeekBar bar) { dragging = false; updateControls(); }
            @Override public void onProgressChanged(SeekBar bar, int progress, boolean fromUser) {
                if (fromUser && player != null) {
                    seekPlayback((int) ((long) player.getDuration() * progress / 1000));
                }
            }
        });
        MediaPlayer.OnPreparedListener onPrepared = prepared -> {
            player = prepared;
            loading.setVisibility(View.GONE);
            enableControls(true);
            if (startWhenReady) startPlayback();
            updateControls();
        };
        MediaPlayer.OnErrorListener onError = (ignored, what, extra) -> {
            player = null;
            loading.setVisibility(View.GONE);
            audio.setVisibility(View.GONE);
            failure.setVisibility(View.VISIBLE);
            enableControls(false);
            return true;
        };
        video.setOnPreparedListener(onPrepared);
        video.setOnCompletionListener(ignored -> updateControls());
        video.setOnErrorListener(onError);
        dialog.setContentView(content);
        dialog.setOnDismissListener(ignored -> nativeClosed(id));
        dialog.show();
        dialog.getWindow().setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        if (audioOnly) {
            // HTTP streaming can estimate OGG duration from its nominal bitrate.
            // Random-access input lets the extractor read its actual end position.
            audioPlayer = new MediaPlayer();
            audioPlayer.setOnPreparedListener(onPrepared);
            audioPlayer.setOnCompletionListener(ignored -> updateControls());
            audioPlayer.setOnErrorListener(onError);
            try {
                audioSource = new HttpMediaSource(url, labels.optLong("size"));
                audioPlayer.setDataSource(audioSource);
                audioPlayer.prepareAsync();
            } catch (java.io.IOException failureToOpen) {
                onError.onError(audioPlayer, 0, 0);
            }
        } else {
            video.setVideoURI(Uri.parse(url));
        }
        handler.post(tick);
    }

    private Button button(Context context, LinearLayout row, String label) {
        Button button = new Button(context);
        button.setAllCaps(false);
        button.setText(labels.optString(label));
        row.addView(button, new LinearLayout.LayoutParams(0, -2, 1));
        return button;
    }

    private void enableControls(boolean enabled) {
        play.setEnabled(enabled);
        restart.setEnabled(enabled);
        mute.setEnabled(enabled);
        seek.setEnabled(enabled);
    }

    private void updateControls() {
        if (player == null) return;
        int duration = Math.max(0, player.getDuration());
        int position = Math.max(0, player.getCurrentPosition());
        if (!dragging && duration > 0) seek.setProgress((int) ((long) position * 1000 / duration));
        time.setText(timestamp(position) + " / " + timestamp(duration));
        play.setText(labels.optString(player.isPlaying() ? "pause" : "play"));
        mute.setText(labels.optString(muted ? "unmute" : "mute"));
    }

    private static String timestamp(int milliseconds) {
        int seconds = milliseconds / 1000;
        if (seconds >= 3600) return String.format(java.util.Locale.ROOT, "%d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60);
        return String.format(java.util.Locale.ROOT, "%d:%02d", seconds / 60, seconds % 60);
    }

    private void startPlayback() {
        if (audioPlayer != null) player.start(); else video.start();
    }

    private void releasePlayback() {
        handler.removeCallbacks(tick);
        player = null;
        if (audioSource != null) audioSource.close();
        if (audioPlayer != null) {
            audioPlayer.release();
            audioPlayer = null;
        }
        video.stopPlayback();
    }

    private void pausePlayback() {
        if (audioPlayer != null) player.pause(); else video.pause();
    }

    private void seekPlayback(int milliseconds) {
        if (audioPlayer != null) player.seekTo(milliseconds); else video.seekTo(milliseconds);
    }

    void pause() { startWhenReady = false; if (player != null) pausePlayback(); updateControls(); }
    void close() { dialog.dismiss(); }
}
