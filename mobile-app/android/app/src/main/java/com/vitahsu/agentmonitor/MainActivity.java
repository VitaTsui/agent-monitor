package com.vitahsu.agentmonitor;

import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;

import com.getcapacitor.BridgeActivity;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;

/**
 * 接收其他 App 分享的文件：拷到应用缓存目录，然后通过 window 事件
 * 通知网页（网页弹会话选择器并走现有上传通道）。
 */
public class MainActivity extends BridgeActivity {

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        handleShare(getIntent());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleShare(intent);
    }

    private void handleShare(Intent intent) {
        if (intent == null || !Intent.ACTION_SEND.equals(intent.getAction())) {
            return;
        }
        Uri uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
        if (uri == null) {
            return;
        }
        try {
            String name = queryName(uri);
            File out = new File(getCacheDir(), "shared-" + System.currentTimeMillis() + "-" + name);
            try (InputStream in = getContentResolver().openInputStream(uri);
                 FileOutputStream fos = new FileOutputStream(out)) {
                byte[] buf = new byte[65536];
                int n;
                while ((n = in.read(buf)) > 0) {
                    fos.write(buf, 0, n);
                }
            }
            String json = "{\"path\":\"" + out.getAbsolutePath().replace("\\", "\\\\")
                + "\",\"name\":\"" + name.replace("\"", "") + "\"}";
            // 网页可能尚未加载完成：延迟触发一次，保证 bridge 就绪
            final String payload = json;
            getBridge().getWebView().postDelayed(
                () -> getBridge().triggerWindowJSEvent("amSharedFile", payload), 1200);
        } catch (Exception e) {
            // 分享失败静默：不阻塞正常启动
        }
    }

    private String queryName(Uri uri) {
        String name = "shared.bin";
        try (Cursor c = getContentResolver().query(uri, null, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (idx >= 0 && c.getString(idx) != null) {
                    name = c.getString(idx);
                }
            }
        } catch (Exception ignored) { }
        return name.replaceAll("[/\\\\]", "_");
    }
}
