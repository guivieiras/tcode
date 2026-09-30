package com.tryanks.tcode;

import android.app.Activity;
import android.app.PendingIntent;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;
import java.io.File;
import java.io.FileInputStream;
import java.io.OutputStream;

/** An explicit user action opens Android's permission and install confirmation flows. */
final class ApkInstaller {
    static final int PERMISSION_REQUEST = 6105;
    private final GpuiActivity activity;
    private long request;
    private File apk;

    ApkInstaller(GpuiActivity activity) { this.activity = activity; }

    void open(long id, String path) {
        if (apk != null) { GpuiActivity.apkResult(id, 2, "An installer request is already pending"); return; }
        request = id;
        apk = new File(path);
        try {
            String root = new File(activity.getFilesDir(), "apks").getCanonicalPath() + File.separator;
            if (!apk.getCanonicalPath().startsWith(root) || !apk.isFile()) {
                throw new IllegalArgumentException("Verified APK is unavailable");
            }
            if (!activity.getPackageManager().canRequestPackageInstalls()) {
                activity.startActivityForResult(new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                        Uri.parse("package:" + activity.getPackageName())), PERMISSION_REQUEST);
            } else { install(); }
        } catch (Exception error) { fail(error.toString()); }
    }

    void permissionResult() {
        if (apk == null) return;
        if (activity.getPackageManager().canRequestPackageInstalls()) install();
        else fail("Permission to install apps was not granted");
    }

    private void fail(String message) {
        GpuiActivity.apkResult(request, 2, message);
        apk = null;
    }

    private void install() {
        final File source = apk;
        final long id = request;
        // Session copying can take seconds; never block Android's main thread.
        new Thread(() -> {
            PackageInstaller installer = activity.getPackageManager().getPackageInstaller();
            int sessionId = -1;
            try {
                PackageInstaller.SessionParams params = new PackageInstaller.SessionParams(
                        PackageInstaller.SessionParams.MODE_FULL_INSTALL);
                params.setAppPackageName(activity.getPackageName());
                params.setSize(source.length());
                if (Build.VERSION.SDK_INT >= 31) {
                    params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED);
                }
                sessionId = installer.createSession(params);
                try (PackageInstaller.Session session = installer.openSession(sessionId)) {
                    // Close the APK stream exactly once, before committing the session.
                    try (FileInputStream input = new FileInputStream(source);
                         OutputStream output = session.openWrite("base.apk", 0, source.length())) {
                        byte[] buffer = new byte[65536];
                        int count;
                        while ((count = input.read(buffer)) != -1) output.write(buffer, 0, count);
                        session.fsync(output);
                    }
                    Intent callback = new Intent(activity, InstallResultReceiver.class)
                            .setAction(activity.getPackageName() + ".INSTALL_RESULT")
                            .putExtra("request_id", id);
                    int flags = PendingIntent.FLAG_UPDATE_CURRENT;
                    if (Build.VERSION.SDK_INT >= 31) flags |= PendingIntent.FLAG_MUTABLE;
                    PendingIntent pending = PendingIntent.getBroadcast(activity, sessionId, callback, flags);
                    session.commit(pending.getIntentSender());
                }
                activity.runOnUiThread(() -> apk = null);
            } catch (Exception error) {
                if (sessionId >= 0) installer.abandonSession(sessionId);
                activity.runOnUiThread(() -> fail(error.toString()));
            }
        }, "apk-installer").start();
    }
}
