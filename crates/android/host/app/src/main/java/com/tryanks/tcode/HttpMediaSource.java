package com.tryanks.tcode;

import android.media.MediaDataSource;
import java.io.DataInputStream;
import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.URL;

/** Random-access audio over the player-owned loopback endpoint, with one bounded cache. */
final class HttpMediaSource extends MediaDataSource {
    private final URL url;
    private final long length;
    private final byte[] cache = new byte[64 * 1024];
    private long cacheStart = -1;
    private int cacheLength;
    private volatile boolean closed;
    private volatile HttpURLConnection active;

    HttpMediaSource(String url, long length) throws IOException {
        this.url = new URL(url);
        this.length = length;
    }

    @Override public long getSize() { return length; }

    @Override public synchronized int readAt(long position, byte[] buffer, int offset, int size) throws IOException {
        if (closed) throw new IOException("Media source closed");
        if (size == 0) return 0;
        if (position >= length) return -1;
        if (position < cacheStart || position >= cacheStart + cacheLength) {
            int count = (int) Math.min(cache.length, length - position);
            HttpURLConnection connection = (HttpURLConnection) url.openConnection();
            connection.setConnectTimeout(10000);
            connection.setReadTimeout(10000);
            connection.setRequestProperty("Range", "bytes=" + position + "-" + (position + count - 1));
            active = connection;
            try {
                if (closed) throw new IOException("Media source closed");
                if (connection.getResponseCode() != 206) throw new IOException("Media range unavailable");
                cacheLength = 0;
                try (DataInputStream input = new DataInputStream(connection.getInputStream())) {
                    input.readFully(cache, 0, count);
                }
                cacheStart = position;
                cacheLength = count;
            } finally {
                connection.disconnect();
                active = null;
            }
        }
        int start = (int) (position - cacheStart);
        int count = Math.min(size, cacheLength - start);
        System.arraycopy(cache, start, buffer, offset, count);
        return count;
    }

    @Override public void close() {
        closed = true;
        HttpURLConnection connection = active;
        if (connection != null) connection.disconnect();
    }
}
