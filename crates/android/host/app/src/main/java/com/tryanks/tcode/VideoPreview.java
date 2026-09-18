package com.tryanks.tcode;

import android.app.Activity;
import android.app.Dialog;
import android.net.Uri;
import android.content.Context;
import android.view.ContextThemeWrapper;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.widget.*;

/** The dialog owns playback; its Rust caller owns the authenticated byte stream. */
final class VideoPreview {
    final long id;
    private final Dialog dialog;
    private final VideoView video;
    private boolean startWhenReady = true;
    private static native void nativeClosed(long id);

    VideoPreview(Activity activity, long id, String url, String title, String error, String close, boolean dark) {
        this.id = id;
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
        header.addView(heading, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
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
        MediaController controls = new MediaController(context);
        controls.setAnchorView(video);
        video.setMediaController(controls);
        video.setOnPreparedListener(player -> {
            loading.setVisibility(View.GONE);
            if (startWhenReady) video.start();
            controls.show();
        });
        video.setOnErrorListener((player, what, extra) -> {
            loading.setVisibility(View.GONE);
            failure.setVisibility(View.VISIBLE);
            controls.hide();
            return true;
        });
        dialog.setContentView(content);
        dialog.setOnDismissListener(ignored -> {
            controls.hide();
            video.stopPlayback();
            nativeClosed(id);
        });
        dialog.show();
        dialog.getWindow().setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        video.setVideoURI(Uri.parse(url));
    }

    void pause() { startWhenReady = false; video.pause(); }
    void close() { dialog.dismiss(); }
}
