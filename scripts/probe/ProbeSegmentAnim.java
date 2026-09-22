import java.awt.Rectangle;
import java.awt.Robot;
import java.awt.image.BufferedImage;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;
import javax.imageio.ImageIO;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

// Evidence collector for segment/navigation selection-state animation.
// Samples the sidebar strip at high frequency while the click POST is in flight,
// so a tween shows intermediate frames while a jump does not.
public final class ProbeSegmentAnim {
    static HttpClient client;
    static String token;

    public static void main(String[] args) throws Exception {
        Path out = Paths.get(args[0]);
        String base = args[1];
        token = args[2];
        int tx = Integer.parseInt(args[3]);
        int ty = Integer.parseInt(args[4]);
        int yOld = Integer.parseInt(args[5]);
        int yNew = Integer.parseInt(args[6]);
        int yInert = Integer.parseInt(args[7]);
        int left = Integer.parseInt(args[8]);
        int top0 = Integer.parseInt(args[9]);
        client = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(5)).build();
        Robot robot = new Robot();

        int top = Math.min(Math.min(yOld, yNew), yInert) - 24;
        int bottom = Math.max(Math.max(yOld, yNew), yInert) + 24;
        Rectangle strip = new Rectangle(left, top0 + top, 200, bottom - top);
        Rectangle whole = new Rectangle(left, top0, 1400, 820);

        BufferedImage b = robot.createScreenCapture(whole);
        ImageIO.write(b, "png", out.resolve("probe-baseline.png").toFile());
        int col = -1;
        for (int x = 16; x <= 178; x++) {
            if (uniform(b, left, x, top0 + yOld - 16, top0 + yOld + 16)
                    && uniform(b, left, x, top0 + yInert - 16, top0 + yInert + 16)) {
                col = x;
                break;
            }
        }
        if (col < 0) throw new IllegalStateException("no uniform background column in sidebar");
        int sampleX = strip.x + col;

        final int fx = sampleX;
        final List<long[]> samples = new ArrayList<>();
        final List<String> saved = new ArrayList<>();
        final AtomicBoolean running = new AtomicBoolean(true);
        Thread cap = new Thread(() -> {
            int lastO = -1, lastN = -1, lastK = -1;
            while (running.get()) {
                long t = System.nanoTime();
                BufferedImage im;
                try {
                    im = robot.createScreenCapture(strip);
                } catch (Exception e) {
                    continue;
                }
                int o = im.getRGB(fx - strip.x, yOld - top);
                int n = im.getRGB(fx - strip.x, yNew - top);
                int k = im.getRGB(fx - strip.x, yInert - top);
                synchronized (samples) {
                    int idx = samples.size();
                    samples.add(new long[] { t, o, n, k });
                    if (o != lastO || n != lastN || k != lastK) {
                        lastO = o;
                        lastN = n;
                        lastK = k;
                        String name = String.format("probe-%05d.png", idx);
                        try {
                            ImageIO.write(im, "png", out.resolve(name).toFile());
                            saved.add(name + "@" + idx);
                        } catch (Exception e) {
                        }
                    }
                }
                try {
                    Thread.sleep(12);
                } catch (InterruptedException e) {
                    return;
                }
            }
        });
        cap.start();
        Thread.sleep(1200);
        long clickStart = System.nanoTime();
        HttpResponse<String> r = post(base + "/action", "{\"type\":\"click\",\"x\":" + tx + ",\"y\":" + ty + "}");
        long clickEnd = System.nanoTime();
        Thread.sleep(5800);
        running.set(false);
        cap.join(2000);

        BufferedImage fin = robot.createScreenCapture(whole);
        ImageIO.write(fin, "png", out.resolve("probe-final.png").toFile());
        String stateAfter = get(base + "/state");

        StringBuilder csv = new StringBuilder();
        csv.append("t_rel_ms,old,new,inert\n");
        synchronized (samples) {
            for (long[] s : samples) {
                csv.append(String.format("%.3f,%08X,%08X,%08X%n",
                        (s[0] - clickStart) / 1e6, s[1], s[2], s[3]));
            }
        }
        Files.writeString(out.resolve("samples.csv"), csv.toString(), StandardCharsets.UTF_8);
        StringBuilder js = new StringBuilder();
        js.append("{");
        js.append("\"col\":").append(col).append(",");
        js.append("\"sampleX\":").append(fx).append(",");
        js.append("\"clickReplyMs\":").append((clickEnd - clickStart) / 1000000).append(",");
        js.append("\"sampleCount\":").append(samples.size()).append(",");
        StringBuilder sl = new StringBuilder();
        for (int i = 0; i < saved.size(); i++) {
            if (i > 0) sl.append(",");
            sl.append("\"").append(saved.get(i)).append("\"");
        }
        js.append("\"savedList\":[").append(sl).append("],");
        js.append("\"clickReply\":\"").append(r.body().replace("\"", "'")).append("\",");
        js.append("\"stateAfter\":\"").append(stateAfter.replace("\"", "'")).append("\"");
        js.append("}");
        Files.writeString(out.resolve("probe-info.json"), js.toString(), StandardCharsets.UTF_8);
        System.out.println(js);
    }

    static boolean uniform(BufferedImage im, int left, int x, int y0, int y1) {
        int c = im.getRGB(left + x, y0);
        for (int y = y0; y <= y1; y++) {
            if (im.getRGB(left + x, y) != c) return false;
        }
        return true;
    }

    static String get(String url) throws Exception {
        HttpRequest rq = HttpRequest.newBuilder(URI.create(url))
                .header("X-HLS-Test-Token", token).timeout(Duration.ofSeconds(20)).build();
        return client.send(rq, HttpResponse.BodyHandlers.ofString()).body();
    }

    static HttpResponse<String> post(String url, String body) throws Exception {
        HttpRequest rq = HttpRequest.newBuilder(URI.create(url))
                .header("Content-Type", "application/json")
                .header("X-HLS-Test-Token", token)
                .POST(HttpRequest.BodyPublishers.ofString(body, StandardCharsets.UTF_8))
                .timeout(Duration.ofSeconds(30)).build();
        return client.send(rq, HttpResponse.BodyHandlers.ofString());
    }
}
