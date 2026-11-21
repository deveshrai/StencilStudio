package io.github.deveshrai.stencilstudio;

import android.app.Activity;
import android.content.ContentValues;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.MediaStore;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Invisible helper that shows the system Photo Picker ("gallery"), the Files app
 * ("files") or the camera ("camera"), and copies the result into the app cache, because a NativeActivity
 * cannot receive activity results itself. The Rust side polls {@link #pending} and
 * reads {@link #result}: an absolute path, or "" when the user cancelled.
 */
public class PickerActivity extends Activity {
    public static volatile boolean pending;
    public static volatile String result;
    private static final int REQUEST = 7;
    /** Where the camera writes its photo (Pictures/StencilStudio). */
    private Uri photo;

    /** Called from Rust (any thread). */
    public static void start(Activity from, String mode, String mime) {
        result = null;
        pending = true;
        Intent i = new Intent(from, PickerActivity.class);
        i.putExtra("mode", mode);
        i.putExtra("mime", mime);
        from.startActivity(i);
    }

    @Override
    protected void onCreate(Bundle saved) {
        setTheme(android.R.style.Theme_Translucent_NoTitleBar);
        super.onCreate(saved);
        if (saved != null) { // recreated while the picker is up: wait for the result
            String p = saved.getString("photo");
            if (p != null) photo = Uri.parse(p);
            return;
        }
        String mode = getIntent().getStringExtra("mode");
        String mime = getIntent().getStringExtra("mime");
        if (mime == null) mime = "*/*";
        Intent pick;
        if ("gallery".equals(mode)) {
            if (Build.VERSION.SDK_INT >= 33) {
                pick = new Intent(MediaStore.ACTION_PICK_IMAGES);
            } else {
                pick = new Intent(Intent.ACTION_PICK);
                pick.setDataAndType(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, "image/*");
            }
        } else if ("camera".equals(mode)) {
            ContentValues v = new ContentValues();
            v.put(MediaStore.Images.Media.DISPLAY_NAME, "StencilStudio-" + System.currentTimeMillis() + ".jpg");
            v.put(MediaStore.Images.Media.MIME_TYPE, "image/jpeg");
            v.put(MediaStore.Images.Media.RELATIVE_PATH, "Pictures/StencilStudio");
            try {
                photo = getContentResolver().insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, v);
            } catch (Exception e) {
                photo = null;
            }
            if (photo == null) { deliver(""); return; }
            pick = new Intent(MediaStore.ACTION_IMAGE_CAPTURE);
            pick.putExtra(MediaStore.EXTRA_OUTPUT, photo);
            pick.addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION | Intent.FLAG_GRANT_READ_URI_PERMISSION);
        } else {
            pick = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            pick.addCategory(Intent.CATEGORY_OPENABLE);
            if (mime.contains(",")) {
                pick.setType("*/*");
                pick.putExtra(Intent.EXTRA_MIME_TYPES, mime.split(","));
            } else {
                pick.setType(mime);
            }
        }
        try {
            startActivityForResult(pick, REQUEST);
        } catch (Exception e) {
            deliver("");
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        if (photo != null) out.putString("photo", photo.toString());
    }

    @Override
    protected void onActivityResult(int request, int code, Intent data) {
        if (photo != null) { // camera: the picture is in `photo`, not in `data`
            final Uri p = photo;
            if (request != REQUEST || code != RESULT_OK) {
                try { getContentResolver().delete(p, null, null); } catch (Exception ignored) { }
                deliver("");
                return;
            }
            new Thread(new Runnable() {
                public void run() { deliver(copy(p)); }
            }).start();
            return;
        }
        if (request != REQUEST || code != RESULT_OK || data == null || data.getData() == null) {
            deliver("");
            return;
        }
        final Uri uri = data.getData();
        new Thread(new Runnable() {
            public void run() { deliver(copy(uri)); }
        }).start();
    }

    private String copy(Uri uri) {
        String name = "picked";
        try (Cursor c = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst() && c.getString(0) != null) name = c.getString(0);
        } catch (Exception ignored) {
        }
        name = name.replaceAll("[/\\\\]", "_");
        if (!name.contains(".")) {
            String type = getContentResolver().getType(uri);
            if (type != null && type.startsWith("image/")) name += "." + type.substring(6).replace("jpeg", "jpg");
        }
        File dir = new File(getCacheDir(), "picked");
        dir.mkdirs();
        File[] old = dir.listFiles();
        if (old != null) for (File f : old) f.delete();
        File out = new File(dir, name);
        try (InputStream in = getContentResolver().openInputStream(uri);
             OutputStream os = new FileOutputStream(out)) {
            byte[] buf = new byte[1 << 16];
            int n;
            while ((n = in.read(buf)) > 0) os.write(buf, 0, n);
            return out.getAbsolutePath();
        } catch (Exception e) {
            return "";
        }
    }

    private void deliver(String path) {
        result = path;
        pending = false;
        runOnUiThread(new Runnable() {
            public void run() { finish(); overridePendingTransition(0, 0); }
        });
    }
}
