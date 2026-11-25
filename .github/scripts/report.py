"""Release notes block: SHA-256, tracker scan (εxodus signatures) and VirusTotal result."""
import glob, hashlib, json, os, re, subprocess, sys, time, urllib.request

apk = sys.argv[1]
sha = hashlib.sha256(open(apk, "rb").read()).hexdigest()
out = ["### Проверка", "", f"SHA-256: `{sha}`", ""]


def get(url, headers=None, data=None, method=None):
    req = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read())


# --- trackers: the same class-name signatures εxodus Privacy uses ---
try:
    tool = glob.glob(os.path.expandvars("$ANDROID_HOME/cmdline-tools/*/bin/apkanalyzer"))[0]
    pkgs = subprocess.run([tool, "dex", "packages", "--defined-only", apk], capture_output=True, text=True, check=True).stdout
    classes = "\n".join(line.split()[-1] for line in pkgs.splitlines() if line.strip())
    trackers = get("https://reports.exodus-privacy.eu.org/api/trackers")["trackers"].values()
    found = sorted({t["name"] for t in trackers if t.get("code_signature") and re.search(t["code_signature"], classes)})
    total = len(list(trackers)) if not isinstance(trackers, list) else len(trackers)
    out.append(
        f"Трекеры: **не найдено** (проверено {total} сигнатур [εxodus](https://reports.exodus-privacy.eu.org/trackers/))"
        if not found
        else "Трекеры: " + ", ".join(found)
    )
except Exception as e:  # noqa: BLE001
    print(f"tracker scan failed: {e}", file=sys.stderr)
    out.append("Трекеры: проверка не удалась")

# --- VirusTotal ---
key = os.environ.get("VT_API_KEY")
link = f"https://www.virustotal.com/gui/file/{sha}"
if key:
    try:
        h = {"x-apikey": key}
        boundary = "----vt" + sha[:16]
        body = (
            f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{os.path.basename(apk)}"\r\n'
            "Content-Type: application/vnd.android.package-archive\r\n\r\n"
        ).encode() + open(apk, "rb").read() + f"\r\n--{boundary}--\r\n".encode()
        res = get(
            "https://www.virustotal.com/api/v3/files",
            {**h, "Content-Type": f"multipart/form-data; boundary={boundary}"},
            body,
            "POST",
        )
        aid = res["data"]["id"]
        stats = None
        for _ in range(40):  # up to ~10 min
            time.sleep(15)
            a = get(f"https://www.virustotal.com/api/v3/analyses/{aid}", h)["data"]["attributes"]
            if a["status"] == "completed":
                stats = a["stats"]
                break
        if stats:
            bad = stats.get("malicious", 0) + stats.get("suspicious", 0)
            engines = sum(stats.get(k, 0) for k in ("malicious", "suspicious", "undetected", "harmless"))
            out.append(f"VirusTotal: **{bad}/{engines}** — [отчёт]({link})")
        else:
            out.append(f"VirusTotal: анализ ещё идёт — [отчёт]({link})")
    except Exception as e:  # noqa: BLE001
        print(f"virustotal failed: {e}", file=sys.stderr)
        out.append(f"VirusTotal: [отчёт]({link})")
else:
    out.append(f"VirusTotal: [проверить]({link})")

print("\n\n".join(line for line in out if line))
