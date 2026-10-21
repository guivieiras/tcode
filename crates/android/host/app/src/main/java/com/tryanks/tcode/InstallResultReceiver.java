package com.tryanks.tcode;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.widget.Toast;

public final class InstallResultReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context context, Intent intent) {
        long id = intent.getLongExtra("request_id", 0);
        int status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE);
        if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            Intent confirmation = intent.getParcelableExtra(Intent.EXTRA_INTENT);
            try {
                if (confirmation == null) throw new IllegalStateException("Android did not supply install confirmation");
                confirmation.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                context.startActivity(confirmation);
                GpuiActivity.apkResult(id, 0, "confirmation_opened");
            } catch (Exception error) { GpuiActivity.apkResult(id, 2, error.toString()); }
        } else if (status == PackageInstaller.STATUS_SUCCESS) {
            GpuiActivity.apkResult(id, 0, "installed");
        } else {
            String message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE);
            if (message == null) message = "Android installation failed (" + status + ")";
            GpuiActivity.apkResult(id, 2, message);
            Toast.makeText(context, message, Toast.LENGTH_LONG).show();
        }
    }
}
