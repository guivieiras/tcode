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
        dialog = new Dialog(context);
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
        body.addView(video, new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER));
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
            if (video.isPlaying()) video.pause();
            else {
                if (video.getCurrentPosition() >= video.getDuration()) video.seekTo(0);
                video.start();
            }
            updateControls();
        });
        restart.setOnClickListener(v -> { video.seekTo(0); video.start(); updateControls(); });
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
                    video.seekTo((int) ((long) video.getDuration() * progress / 1000));
                }
            }
        });
        video.setOnPreparedListener(prepared -> {
            player = prepared;
            loading.setVisibility(View.GONE);
            enableControls(true);
            if (startWhenReady) video.start();
            updateControls();
        });
        video.setOnCompletionListener(ignored -> updateControls());
        video.setOnErrorListener((ignored, what, extra) -> {
            player = null;
            loading.setVisibility(View.GONE);
            failure.setVisibility(View.VISIBLE);
            enableControls(false);
            return true;
        });
        dialog.setContentView(content);
        dialog.setOnDismissListener(ignored -> {
            handler.removeCallbacks(tick);
            player = null;
            video.stopPlayback();
            nativeClosed(id);
        });
        dialog.show();
        dialog.getWindow().setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        video.setVideoURI(Uri.parse(url));
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
        int duration = Math.max(0, video.getDuration());
        int position = Math.max(0, video.getCurrentPosition());
        if (!dragging && duration > 0) seek.setProgress((int) ((long) position * 1000 / duration));
        time.setText(timestamp(position) + " / " + timestamp(duration));
        play.setText(labels.optString(video.isPlaying() ? "pause" : "play"));
        mute.setText(labels.optString(muted ? "unmute" : "mute"));
    }

    private static String timestamp(int milliseconds) {
        int seconds = milliseconds / 1000;
        if (seconds >= 3600) return String.format(java.util.Locale.ROOT, "%d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60);
        return String.format(java.util.Locale.ROOT, "%d:%02d", seconds / 60, seconds % 60);
    }

    void pause() { startWhenReady = false; video.pause(); updateControls(); }
    void close() { dialog.dismiss(); }
}
