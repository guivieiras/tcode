package com.tryanks.tcode;

import android.test.AndroidTestCase;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicReference;

/** Seeks must return host bytes, including cache misses and the final short range. */
@SuppressWarnings("deprecation")
public final class HttpMediaSourceTest extends AndroidTestCase {
    public void testBoundedReadsSeekAndClose() throws Exception {
        byte[] file = new byte[150001];
        for (int i = 0; i < file.length; i++) file[i] = (byte) (i % 251);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        try (ServerSocket server = new ServerSocket(0);
             HttpMediaSource source = new HttpMediaSource("http://127.0.0.1:" + server.getLocalPort() + "/audio", file.length)) {
            Thread worker = new Thread(() -> {
                try {
                    while (!server.isClosed()) {
                        try (Socket socket = server.accept()) {
                            BufferedReader input = new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII));
                            String line;
                            int start = -1, end = -1;
                            while ((line = input.readLine()) != null && !line.isEmpty()) {
                                if (line.startsWith("Range: bytes=")) {
                                    String[] range = line.substring(13).split("-");
                                    start = Integer.parseInt(range[0]);
                                    end = Integer.parseInt(range[1]);
                                }
                            }
                            assertTrue(start >= 0 && end < file.length && end - start < 64 * 1024);
                            String headers = "HTTP/1.1 206 Partial Content\r\nContent-Length: " + (end - start + 1)
                                    + "\r\nContent-Range: bytes " + start + "-" + end + "/" + file.length + "\r\nConnection: close\r\n\r\n";
                            socket.getOutputStream().write(headers.getBytes(StandardCharsets.US_ASCII));
                            socket.getOutputStream().write(file, start, end - start + 1);
                        }
                    }
                } catch (Throwable error) {
                    if (!server.isClosed()) failure.set(error);
                }
            });
            worker.start();
            assertEquals(file.length, source.getSize());
            byte[] actual = new byte[128];
            for (int position : new int[] {0, 100, 90000, 149950, 10}) {
                int count = source.readAt(position, actual, 0, actual.length);
                assertEquals(Math.min(actual.length, file.length - position), count);
                for (int i = 0; i < count; i++) assertEquals(file[position + i], actual[i]);
            }
            assertEquals(-1, source.readAt(file.length, actual, 0, actual.length));
            source.close();
            try {
                source.readAt(0, actual, 0, 1);
                fail("closed audio sources must reject reads");
            } catch (IOException expected) { }
            server.close();
            worker.join(1000);
            assertFalse(worker.isAlive());
            assertNull(failure.get());
        }
    }
}
