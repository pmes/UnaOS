// RegExp: tokenizing, validating and rewriting a synthetic log with a mix of patterns.
var levels = ["INFO", "WARN", "ERROR", "DEBUG"], hosts = ["alpha.example.com", "beta.example.org", "gamma.test"];
var lines = [];
for (var i = 0; i < 4000; i++) {
  lines.push("2024-0" + (1 + i % 9) + "-1" + (i % 10) + "T12:" + (10 + i % 50) + ":00Z [" + levels[i % 4] + "] host=" + hosts[i % 3] +
    " user=u" + (i * 7 % 1000) + " msg=\"request " + i + " took " + (i % 977) + "ms\" ip=10.0." + (i % 256) + "." + (i * 3 % 256));
}
var log = lines.join("\n");
var re = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})Z \[(\w+)\] host=([\w.]+) user=(\w+) msg="([^"]*)" ip=((?:\d{1,3}\.){3}\d{1,3})$/gm;
var counts = {}, total = 0, m;
for (var rep = 0; rep < 3; rep++) {
  re.lastIndex = 0;
  while ((m = re.exec(log)) !== null) { counts[m[7]] = (counts[m[7]] || 0) + 1; total += parseInt(m[10].match(/took (\d+)ms/)[1], 10); }
}
var masked = log.replace(/\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b/g, "$1.$2.x.x").replace(/user=u(\d+)/g, function (s, n) { return "user=#" + (n.length); });
var words = log.split(/\s+/).filter(function (w) { return /^[a-z]+=/.test(w); }).length;
var named = 0; for (var mm of log.matchAll(/(?<lvl>ERROR|WARN)\] host=(?<h>[a-z]+)/g)) if (mm.groups.h === "alpha") named++;
print("regexp " + JSON.stringify(counts) + " " + total + " " + masked.length + " " + words + " " + named);
