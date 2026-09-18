package com.tryanks.tcode;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.service.notification.StatusBarNotification;
import android.util.Log;
import java.util.HashSet;
import java.util.Set;
import java.util.UUID;

/** Native delivery only. The GPUI shell owns eligibility and thread navigation. */
final class SystemNotifications {
    private static final String CHANNEL = "thread-attention";
    private static final String SCHEME = "tcode-notification";
    private final Context context;
    private final NotificationManager manager;
    private final String instance = UUID.randomUUID().toString();
    private final Set<String> delivered = new HashSet<>();

    SystemNotifications(Context context) {
        this.context = context;
        manager = context.getSystemService(NotificationManager.class);
        if (manager == null) return;
        NotificationChannel channel = new NotificationChannel(CHANNEL,
                context.getString(R.string.thread_notifications), NotificationManager.IMPORTANCE_HIGH);
        channel.setSound(null, null);
        channel.enableVibration(false);
        manager.createNotificationChannel(channel);
        // Shell tags belong to the previous activity's attachment and cannot be reopened.
        for (StatusBarNotification notification : manager.getActiveNotifications()) {
            if (CHANNEL.equals(notification.getNotification().getChannelId())) {
                manager.cancel(notification.getTag(), notification.getId());
            }
        }
    }

    boolean show(String tag, String title, String body) {
        if (manager == null || !manager.areNotificationsEnabled()) return false;
        Notification notification = new Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_notification)
                .setContentTitle(title)
                .setContentText(body)
                .setContentIntent(PendingIntent.getActivity(context, 0, intent(tag),
                        PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE))
                .setAutoCancel(true)
                .setVisibility(Notification.VISIBILITY_PRIVATE)
                .build();
        try {
            manager.notify(tag, 0, notification);
            delivered.add(tag);
            return true;
        } catch (SecurityException error) {
            // Permission can be revoked between the permission check and delivery.
            Log.w("Tcode-GPUI", "Notification permission unavailable", error);
            return false;
        }
    }

    Intent intent(String tag) {
        return new Intent(context, GpuiActivity.class)
                .setAction(Intent.ACTION_VIEW)
                .setData(new Uri.Builder().scheme(SCHEME).authority(instance).appendPath(tag).build())
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
    }

    String takeResponse(Intent intent) {
        Uri data = intent.getData();
        if (data == null || !SCHEME.equals(data.getScheme())
                || !instance.equals(data.getAuthority()) || data.getPathSegments().size() != 1) {
            return null;
        }
        String tag = data.getPathSegments().get(0);
        if (!delivered.remove(tag)) return null;
        if (manager != null) manager.cancel(tag, 0);
        return tag;
    }

    void dismiss(String tag) {
        delivered.remove(tag);
        if (manager != null) manager.cancel(tag, 0);
    }

    void close() {
        if (manager != null) {
            for (String tag : delivered) manager.cancel(tag, 0);
        }
        delivered.clear();
    }
}
