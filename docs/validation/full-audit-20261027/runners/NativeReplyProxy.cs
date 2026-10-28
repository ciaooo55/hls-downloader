// Sandbox-only fault injection at the real Native Host reply boundary.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Web.Script.Serialization;

public static class NativeReplyProxy {
    static void Trace(string phase, object metadata) {
        var record = new Dictionary<string, object> {
            {"phase", phase}, {"metadata", metadata}, {"utc", DateTime.UtcNow.ToString("o")}
        };
        string path = Path.Combine(Environment.GetEnvironmentVariable("HLS_REPLY_PROXY_STATE"), "proxy-" + Process.GetCurrentProcess().Id + ".jsonl");
        File.AppendAllText(path, new JavaScriptSerializer().Serialize(record) + "\n");
    }
    static byte[] ReadFrame(Stream stream) {
        byte[] header = new byte[4];
        int first = stream.ReadByte();
        if (first < 0) return null;
        header[0] = (byte)first;
        ReadExactly(stream, header, 1, 3);
        int length = BitConverter.ToInt32(header, 0);
        if (length < 1 || length > 4 * 1024 * 1024) throw new InvalidDataException("Invalid frame");
        byte[] body = new byte[length];
        ReadExactly(stream, body, 0, length);
        return body;
    }
    static void ReadExactly(Stream stream, byte[] buffer, int offset, int length) {
        while (length > 0) {
            int read = stream.Read(buffer, offset, length);
            if (read == 0) throw new EndOfStreamException();
            offset += read;
            length -= read;
        }
    }
    static void WriteFrame(Stream stream, byte[] body) {
        byte[] header = BitConverter.GetBytes(body.Length);
        stream.Write(header, 0, header.Length);
        stream.Write(body, 0, body.Length);
        stream.Flush();
    }
    public static void Main() {
      try {
        if (Environment.UserName != "WDAGUtilityAccount") throw new InvalidOperationException("Sandbox only");
        Trace("started", null);
        var start = new ProcessStartInfo(Environment.GetEnvironmentVariable("HLS_REPLY_REAL_HOST"));
        start.UseShellExecute = false;
        start.CreateNoWindow = true;
        start.RedirectStandardInput = true;
        start.RedirectStandardOutput = true;
        start.RedirectStandardError = true;
        var json = new JavaScriptSerializer();
        json.MaxJsonLength = 4 * 1024 * 1024;
        using (var child = Process.Start(start)) {
            child.ErrorDataReceived += delegate(object sender, DataReceivedEventArgs data) {
                if (data.Data != null) File.AppendAllText(Path.Combine(Environment.GetEnvironmentVariable("HLS_REPLY_PROXY_STATE"), "host-stderr.txt"), data.Data + "\n");
            };
            child.BeginErrorReadLine();
            try {
                var input = Console.OpenStandardInput();
                var output = Console.OpenStandardOutput();
                byte[] body;
                while ((body = ReadFrame(input)) != null) {
                    var request = json.Deserialize<Dictionary<string, object>>(Encoding.UTF8.GetString(body));
                    Trace("forwarded", new Dictionary<string, object> {
                        {"op", request["op"]}, {"request_id", request["__request_id"]}
                    });
                    WriteFrame(child.StandardInput.BaseStream, body);
                    byte[] reply = ReadFrame(child.StandardOutput.BaseStream);
                    if (reply == null) {
                        child.WaitForExit(1000);
                        throw new EndOfStreamException("Real Host closed; exit=" + (child.HasExited ? child.ExitCode.ToString() : "running"));
                    }
                    var response = json.Deserialize<Dictionary<string, object>>(Encoding.UTF8.GetString(reply));
                    if (request.ContainsKey("op") && (string)request["op"] == "offer") {
                        var resource = (Dictionary<string, object>)request["resource"];
                        string key = (string)resource["client_request_id"];
                        string digest;
                        using (var sha = SHA256.Create()) digest = BitConverter.ToString(sha.ComputeHash(body)).Replace("-", "").ToLowerInvariant();
                        string state = Environment.GetEnvironmentVariable("HLS_REPLY_PROXY_STATE");
                        string marker = Path.Combine(state, "fault-injected");
                        bool accepted = response.ContainsKey("ok") && (bool)response["ok"];
                        bool inject = accepted && !File.Exists(marker);
                        if (inject) File.WriteAllText(marker, key);
                        var handoff = response.ContainsKey("handoff") ? (Dictionary<string, object>)response["handoff"] : null;
                        var record = new Dictionary<string, object> {
                            {"client_request_id", key}, {"request_sha256", digest},
                            {"handoff_id", handoff == null ? null : handoff["id"]}, {"injected", inject},
                            {"ok", accepted}, {"error", response.ContainsKey("error") ? response["error"] : null}
                        };
                        File.AppendAllText(Path.Combine(state, "offers.jsonl"), json.Serialize(record) + "\n");
                        if (inject) {
                            reply = Encoding.UTF8.GetBytes(json.Serialize(new Dictionary<string, object> {
                                {"ok", false}, {"error", "Core pipe read: injected after durable offer"},
                                {"__request_id", request["__request_id"]}
                            }));
                            File.WriteAllBytes(Path.Combine(state, "injected-reply.json"), reply);
                        }
                    }
                    WriteFrame(output, reply);
                    Trace("returned", new Dictionary<string, object> {
                        {"op", request["op"]}, {"request_id", request["__request_id"]}
                    });
                }
            } finally {
                if (!child.HasExited) child.Kill();
                child.WaitForExit(5000);
            }
        }
      } catch (Exception error) {
          File.AppendAllText(Path.Combine(Environment.GetEnvironmentVariable("HLS_REPLY_PROXY_STATE"), "proxy-errors.txt"), error.ToString() + "\n");
          Console.Error.WriteLine(error.Message);
          Environment.ExitCode = 1;
      }
    }
}
