package com.tryanks.tcode;

import android.content.ClipboardManager;
import android.content.Context;
import android.test.ActivityInstrumentationTestCase2;
import android.text.Selection;
import android.text.SpannableStringBuilder;
import android.view.View;
import java.util.ArrayList;

/** Real Android clipboard access requires a foreground activity. */
@SuppressWarnings("deprecation")
public final class GpuiClipboardTest extends ActivityInstrumentationTestCase2<GpuiActivity> {
    public GpuiClipboardTest() { super(GpuiActivity.class); }

    public void testClipboardCommandsPublishUnicodeEditsAndRespectAccess() {
        GpuiActivity activity = getActivity();
        getInstrumentation().waitForIdleSync();
        getInstrumentation().runOnMainSync(() -> {
            SpannableStringBuilder text = new SpannableStringBuilder("😀 example here");
            Selection.setSelection(text, 10, 3);
            ArrayList<GpuiInputConnection.State> sent = new ArrayList<>();
            GpuiInputConnection input = new GpuiInputConnection(new View(activity), text, sent::add);
            ClipboardManager clipboard = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);

            assertTrue(input.performContextMenuAction(android.R.id.copy));
            assertEquals("example", clipboard.getPrimaryClip().getItemAt(0).getText().toString());
            assertTrue(sent.isEmpty());
            assertTrue(input.performContextMenuAction(android.R.id.cut));
            assertEquals(new GpuiInputConnection.State("😀  here", 3, 3, -1, -1), sent.get(0));
            assertEquals(1, sent.size());
            assertTrue(input.performContextMenuAction(android.R.id.paste));
            assertEquals(new GpuiInputConnection.State("😀 example here", 10, 10, -1, -1), sent.get(1));
            assertEquals(2, sent.size());
            assertTrue(input.performContextMenuAction(android.R.id.selectAll));
            assertEquals(new GpuiInputConnection.State("😀 example here", 0, 15, -1, -1), sent.get(2));

            input.setClipboardAccess(false, false, false);
            assertFalse(input.performContextMenuAction(android.R.id.copy));
            assertFalse(input.performContextMenuAction(android.R.id.cut));
            assertFalse(input.performContextMenuAction(android.R.id.paste));
            assertEquals(3, sent.size());
            assertEquals("😀 example here", text.toString());
            input.closeConnection();
            assertFalse(input.performContextMenuAction(android.R.id.selectAll));
        });
    }
}
