package com.tryanks.tcode;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.text.Editable;
import android.text.Selection;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import java.util.function.Consumer;

/** Publish Android's completed edits, including selection and composing-span changes. */
class GpuiInputConnection extends BaseInputConnection {
    record State(String text, int selectionStart, int selectionEnd, int composingStart, int composingEnd) {}
    private final Editable editable;
    private final Consumer<State> changed;
    private int batchDepth;
    private boolean closed;
    private State lastState;
    private final Context context;
    private boolean canCopy = true;
    private boolean canCut = true;
    private boolean canPaste = true;

    GpuiInputConnection(View view, Editable editable, Consumer<State> changed) {
        super(view, true);
        this.editable = editable;
        this.changed = changed;
        this.context = view.getContext();
        rememberState();
    }

    @Override public Editable getEditable() { return closed ? null : editable; }

    void setClipboardAccess(boolean copy, boolean cut, boolean paste) {
        canCopy = copy;
        canCut = cut;
        canPaste = paste;
    }

    /** Keyboard editing commands and the floating toolbar share the same edit transaction. */
    @Override public boolean performContextMenuAction(int id) {
        if (closed) return false;
        int start = Math.min(Selection.getSelectionStart(editable), Selection.getSelectionEnd(editable));
        int end = Math.max(Selection.getSelectionStart(editable), Selection.getSelectionEnd(editable));
        if (start < 0) return false;
        ClipboardManager clipboard = (ClipboardManager) context.getSystemService(Context.CLIPBOARD_SERVICE);
        switch (id) {
            case android.R.id.selectAll:
                return setSelection(0, editable.length());
            case android.R.id.copy:
            case android.R.id.cut:
                if (start == end || (id == android.R.id.copy ? !canCopy : !canCut)) return false;
                clipboard.setPrimaryClip(ClipData.newPlainText("tcode", editable.subSequence(start, end).toString()));
                if (id == android.R.id.cut) {
                    beginBatchEdit();
                    finishComposingText();
                    editable.delete(start, end);
                    Selection.setSelection(editable, start);
                    endBatchEdit();
                }
                return true;
            case android.R.id.paste:
                if (!canPaste) return false;
                ClipData clip = clipboard.getPrimaryClip();
                if (clip == null || clip.getItemCount() == 0) return false;
                CharSequence text = clip.getItemAt(0).coerceToText(context);
                if (text == null) return false;
                beginBatchEdit();
                finishComposingText();
                setSelection(start, end);
                commitText(text, 1);
                endBatchEdit();
                return true;
            default:
                return false;
        }
    }

    @Override public void closeConnection() {
        // restartInput retires the old connection after the app has supplied its new state.
        // BaseInputConnection.closeConnection would clear that new composing region.
        closed = true;
    }

    @Override public boolean beginBatchEdit() {
        batchDepth++;
        return true;
    }

    @Override public boolean endBatchEdit() {
        if (batchDepth == 0) return false;
        batchDepth--;
        publishChanges();
        return batchDepth > 0;
    }

    @Override public boolean setSelection(int start, int end) {
        boolean result = super.setSelection(start, end);
        publishChanges();
        return result;
    }

    void publishChanges() {
        if (closed || batchDepth != 0) return;
        State state = readState();
        if (!state.equals(lastState)) {
            lastState = state;
            changed.accept(state);
        }
    }

    void rememberState() { lastState = readState(); }

    private State readState() {
        return new State(editable.toString(), Selection.getSelectionStart(editable),
                Selection.getSelectionEnd(editable), getComposingSpanStart(editable),
                getComposingSpanEnd(editable));
    }
}
