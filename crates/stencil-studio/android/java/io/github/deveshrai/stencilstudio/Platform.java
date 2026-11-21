package io.github.deveshrai.stencilstudio;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.media.MediaScannerConnection;
import android.net.Uri;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.os.ParcelFileDescriptor;
import android.print.PageRange;
import android.print.PrintAttributes;
import android.print.PrintDocumentAdapter;
import android.print.PrintDocumentInfo;
import android.print.PrintManager;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/** System services called from Rust: printing and opening files in other apps. */
public class Platform {
    /** Show the Android print dialog for a PDF file (printers, or "Save as PDF"). */
    public static void printPdf(final Activity a, final String path, final String job) {
        a.runOnUiThread(new Runnable() {
            public void run() {
                PrintManager pm = (PrintManager) a.getSystemService(Context.PRINT_SERVICE);
                pm.print(job, new PrintDocumentAdapter() {
                    @Override
                    public void onLayout(PrintAttributes oldAttrs, PrintAttributes newAttrs, CancellationSignal cancel,
                                         LayoutResultCallback cb, Bundle extras) {
                        if (cancel.isCanceled()) { cb.onLayoutCancelled(); return; }
                        cb.onLayoutFinished(new PrintDocumentInfo.Builder(job + ".pdf")
                                .setContentType(PrintDocumentInfo.CONTENT_TYPE_DOCUMENT).build(), true);
                    }

                    @Override
                    public void onWrite(PageRange[] pages, ParcelFileDescriptor dest, CancellationSignal cancel,
                                        WriteResultCallback cb) {
                        try (InputStream in = new FileInputStream(path);
                             OutputStream out = new FileOutputStream(dest.getFileDescriptor())) {
                            byte[] buf = new byte[1 << 16];
                            int n;
                            while ((n = in.read(buf)) > 0) out.write(buf, 0, n);
                            cb.onWriteFinished(new PageRange[] {PageRange.ALL_PAGES});
                        } catch (Exception e) {
                            cb.onWriteFailed(e.toString());
                        }
                    }
                }, null);
            }
        });
    }

    /** Index a file we wrote to shared storage and open it in a viewer app. */
    public static void openFile(final Activity a, String path, final String mime) {
        MediaScannerConnection.scanFile(a, new String[] {path}, new String[] {mime},
            new MediaScannerConnection.OnScanCompletedListener() {
                public void onScanCompleted(String p, final Uri uri) {
                    if (uri == null) return;
                    a.runOnUiThread(new Runnable() {
                        public void run() {
                            Intent v = new Intent(Intent.ACTION_VIEW);
                            v.setDataAndType(uri, mime);
                            v.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                            try {
                                a.startActivity(Intent.createChooser(v, "Open with"));
                            } catch (ActivityNotFoundException ignored) {
                            }
                        }
                    });
                }
            });
    }
}
