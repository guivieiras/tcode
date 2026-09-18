package com.tryanks.tcode;

import android.app.Notification;
import android.app.NotificationManager;
import android.content.Intent;
import android.service.notification.StatusBarNotification;
import android.test.AndroidTestCase;

/** Run with POST_NOTIFICATIONS granted; exercises Tcode's real notification adapter. */
@SuppressWarnings("deprecation")
public final class SystemNotificationsTest extends AndroidTestCase {
    public void testReplacementDismissalAndResponsesKeepThreadIdentity() {
        SystemNotifications notifications = new SystemNotifications(getContext());
        NotificationManager manager = getContext().getSystemService(NotificationManager.class);
        assertTrue("Grant POST_NOTIFICATIONS before this test", manager.areNotificationsEnabled());
        try {
            assertTrue(notifications.show("thread:1:FB", "Answer needed", "First thread"));
            assertTrue(notifications.show("thread:1:Ea", "Approval needed", "Second thread"));
            Intent first = notifications.intent("thread:1:FB");
            Intent second = notifications.intent("thread:1:Ea");
            assertFalse(first.filterEquals(second));
            assertTrue(notifications.show("thread:1:FB", "Thread completed", "Renamed thread"));
            awaitCount(manager, 2);
            Notification replacement = awaitTitle(manager, "thread:1:FB", "Thread completed");
            assertEquals("Renamed thread", replacement.extras.getString(Notification.EXTRA_TEXT));
            assertEquals("thread:1:Ea", notifications.takeResponse(second));
            assertNull(notifications.takeResponse(second));
            notifications.dismiss("thread:1:FB");
            assertNull(notifications.takeResponse(first));
            awaitCount(manager, 0);
        } finally {
            notifications.close();
        }
    }

    public void testDeliveryFollowsNotificationPermission() {
        SystemNotifications notifications = new SystemNotifications(getContext());
        NotificationManager manager = getContext().getSystemService(NotificationManager.class);
        boolean allowed = manager.areNotificationsEnabled();
        try {
            assertEquals(allowed, notifications.show("permission-check", "Answer needed", "Thread"));
            awaitCount(manager, allowed ? 1 : 0);
            assertEquals(allowed ? "permission-check" : null,
                    notifications.takeResponse(notifications.intent("permission-check")));
        } finally {
            notifications.close();
        }
    }

    private static Notification awaitTitle(NotificationManager manager, String tag, String title) {
        long deadline = android.os.SystemClock.uptimeMillis() + 5000;
        do {
            for (StatusBarNotification entry : manager.getActiveNotifications()) {
                if (tag.equals(entry.getTag()) && title.equals(entry.getNotification().extras
                        .getString(Notification.EXTRA_TITLE))) return entry.getNotification();
            }
            android.os.SystemClock.sleep(20);
        } while (android.os.SystemClock.uptimeMillis() < deadline);
        fail("Replacement notification did not arrive");
        return null;
    }

    private static StatusBarNotification[] awaitCount(NotificationManager manager, int count) {
        long deadline = android.os.SystemClock.uptimeMillis() + 5000;
        StatusBarNotification[] active;
        do {
            active = manager.getActiveNotifications();
            if (active.length == count) return active;
            android.os.SystemClock.sleep(20);
        } while (android.os.SystemClock.uptimeMillis() < deadline);
        assertEquals(count, active.length);
        return active;
    }

    public void testPreviousActivityNotificationCannotOpenANewAttachment() {
        SystemNotifications old = new SystemNotifications(getContext());
        assertTrue("Grant POST_NOTIFICATIONS before this test",
                old.show("thread:1:same", "Answer needed", "Old host"));
        Intent stale = old.intent("thread:1:same");
        SystemNotifications current = new SystemNotifications(getContext());
        try {
            assertTrue(current.show("thread:1:same", "Answer needed", "New host"));
            assertNull(current.takeResponse(stale));
            assertNull(current.takeResponse(new Intent()));
            assertEquals("thread:1:same", current.takeResponse(current.intent("thread:1:same")));
        } finally {
            current.close();
        }
    }
}
