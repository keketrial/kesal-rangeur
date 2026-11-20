#!/usr/bin/env python3 -I
"""Test d'intégration du serveur MCP de kesal-rangeur (bibliothèque standard seulement).

Lance `kesal-rangeur mcp` (stdio) contre une instance BAC À SABLE de kesal-rangeur déjà démarrée, par ex. :

    KESAL_BIND=127.0.0.1:3061 target/debug/kesal-rangeur serve /chemin/bac-a-sable/config.toml &
    python3 -I tests/mcp_integration.py --bin target/debug/kesal-rangeur --url http://127.0.0.1:3061

Ne jamais viser une vraie instance : le test change le nommage (puis le remet) et exige que l'essai à blanc soit ACTIF
au départ (sinon il s'arrête sans rien faire).
"""
import argparse
import json
import os
import queue
import subprocess
import sys
import threading

ap = argparse.ArgumentParser()
ap.add_argument("--bin", default="target/debug/kesal-rangeur")
ap.add_argument("--url", required=True, help="adresse du serveur bac à sable, ex. http://127.0.0.1:3061")
args = ap.parse_args()

env = dict(os.environ, KESAL_URL=args.url)
proc = subprocess.Popen([args.bin, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
out_q = queue.Queue()
stderr_lines = []
threading.Thread(target=lambda: [out_q.put(l) for l in proc.stdout], daemon=True).start()
threading.Thread(target=lambda: [stderr_lines.append(l) for l in proc.stderr], daemon=True).start()

results = []
_id = [0]


def check(name, cond, detail=""):
    results.append(bool(cond))
    print(("PASS " if cond else "FAIL ") + name + ("" if cond else "  -> " + str(detail)[:400]))


def send(obj):
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def request(method, params=None, timeout=90):
    _id[0] += 1
    msg = {"jsonrpc": "2.0", "id": _id[0], "method": method}
    if params is not None:
        msg["params"] = params
    send(msg)
    line = out_q.get(timeout=timeout)
    resp = json.loads(line)  # tout ce qui sort sur stdout DOIT être du JSON
    assert resp.get("jsonrpc") == "2.0" and resp.get("id") == _id[0], resp
    return resp


def call(name, arguments=None, timeout=90):
    r = request("tools/call", {"name": name, "arguments": arguments or {}}, timeout)
    res = r["result"]
    return res["isError"], res["content"][0]["text"]


def call_json(name, arguments=None, timeout=90):
    err, text = call(name, arguments, timeout)
    return err, (text if err else json.loads(text))


try:
    # 1. initialize / tools/list
    r = request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}})
    check("initialize", r["result"]["serverInfo"]["name"] == "kesal-rangeur" and r["result"]["protocolVersion"] == "2025-06-18", r)
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    r = request("tools/list")
    names = {t["name"] for t in r["result"]["tools"]}
    want = {"status", "get_settings", "configure", "set_dry_run", "test_tmdb", "test_services", "browse_folders", "scan", "plan", "search_tmdb", "choose", "preview_naming", "history", "undo", "run"}
    check("tools/list contient les 15 outils", want <= names, want - names)
    check("méthode inconnue -> -32601", request("nope/nope")["error"]["code"] == -32601)
    r = request("resources/read", {"uri": "kesal://help"})
    check("resource kesal://help", "## " in r["result"]["contents"][0]["text"])
    r = request("prompts/get", {"name": "setup_kesal_rangeur"})
    check("prompt setup_kesal_rangeur", "DRY-RUN" in r["result"]["messages"][0]["content"]["text"])

    # 2. status : sécurité du test (jamais sans essai à blanc)
    err, st = call_json("status")
    check("status", not err and st["tmdb_configured"] is True, st)
    if not st.get("dry_run"):
        print("ABANDON : l'essai à blanc n'est pas actif sur", args.url)
        sys.exit(2)
    check("status : dry_run actif", st["dry_run"] is True)

    # 3. configure : changement de préréglage de nommage, rien d'autre ne bouge
    err, before = call_json("get_settings")
    check("get_settings", not err and before["essai"] is True and "secrets" in before, before)
    err, res = call_json("configure", {"naming": "plex"})
    check("configure naming=plex", not err and res["saved"] and res["changed"] == ["naming"], res)
    err, after = call_json("get_settings")
    check("nommage changé", after["naming"]["preset"] == "plex", after.get("naming"))
    same = all(after[k] == before[k] for k in ["language", "inputs", "destinations", "film_libraries", "rules", "copy_method", "scan_interval_min", "auto_range", "essai"])
    check("le reste des réglages est inchangé (dont essai)", same)
    err, text = call("configure", {"essai": False})
    check("configure refuse essai", err and "set_dry_run" in text, text)
    err, text = call("configure", {"naming": "turbo"})
    check("configure refuse un préréglage inconnu", err, text)
    err, st = call_json("status")
    check("essai toujours actif après configure", st["dry_run"] is True)

    # 4. preview_naming
    err, pv = call_json("preview_naming", {"naming": "plex"})
    check("preview_naming plex", not err and "movie_example" in pv and "series_example" in pv, pv)
    err, text = call("preview_naming", {"naming": {"preset": "custom", "movie": "{zzz}", "episode": "{n}"}})
    check("preview_naming variable inconnue -> erreur", err, text)

    # 5. TMDB
    err, text = call("test_tmdb")
    check("test_tmdb", not err and '"ok": true' in text, text)
    err, sr = call_json("search_tmdb", {"q": "Poker Face", "kind": "tv"})
    check("search_tmdb", not err and len(sr["results"]) > 0 and "tmdb_id" in sr["results"][0], sr)

    # 6. browse_folders
    err, fs = call_json("browse_folders", {"path": os.path.dirname(before["inputs"][0]["path"])})
    check("browse_folders", not err and "in" in fs["sub_folders"], fs)

    # 7. scan + plan
    err, sc = call_json("scan", timeout=120)
    check("scan terminé", not err and sc["scan"] == "finished" and sc["total_items"] > 0, sc)
    err, pl = call_json("plan", {"limit": 5})
    check("plan : compteurs + résumé court", not err and pl["total"] == sc["total_items"] and len(pl["items"]) <= 5 and "decision" in pl["items"][0], pl)
    err, pr = call_json("plan", {"filter": "ranger", "limit": 3})
    check("plan filter=ranger", not err and all(i["decision"] == "ranger" for i in pr["items"]) and (not pr["items"] or "destination" in pr["items"][0]), pr)
    err, text = call("plan", {"filter": "nimporte"})
    check("plan filtre invalide -> erreur", err, text)
    check("plan reste concis", len(json.dumps(pl)) < 12000, len(json.dumps(pl)))

    # 8. garde-fous de l'essai à blanc
    err, text = call("set_dry_run", {"enabled": False})
    check("set_dry_run(false) sans phrase -> refusé", err and "Refused" in text, text)
    err, text = call("set_dry_run", {"enabled": False, "confirm": "oui"})
    check("set_dry_run(false) mauvaise phrase -> refusé", err and "Refused" in text, text)
    err, st = call_json("status")
    check("essai toujours actif après les refus", st["dry_run"] is True)
    err, text = call("run")
    check("run refusé en essai (403 clair)", err and "403" in text and "Dry-run" in text, text)
    err, text = call("set_dry_run", {"enabled": True})
    check("set_dry_run(true) sans confirmation", not err and "ON" in text, text)
    err, h = call_json("history")
    check("history vide en essai", not err and h["total"] == 0, h)

    # 9. remise en état
    err, res = call_json("configure", {"naming": "keep"})
    check("remise du nommage", not err and res["saved"], res)
finally:
    try:
        proc.stdin.close()
        proc.wait(timeout=5)
    except Exception:
        proc.kill()

# stdout n'a produit que du JSON (sinon json.loads aurait échoué) ; stderr ne doit jamais contenir de secret
check("aucun secret dans les journaux stderr", not any("eyJ" in l for l in stderr_lines))
print("\n%d/%d vérifications OK" % (sum(results), len(results)))
sys.exit(0 if all(results) else 1)
