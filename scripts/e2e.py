#!/usr/bin/env python3
"""End-to-end test of the indexer + API against a live network (devnet by default).

Runs in an isolated Postgres schema (`e2e`, recreated each run) on separate ports, so a
dev instance can keep running. Requires: built debug binaries, .env with GRPC_ENDPOINTS /
GRPC_X_TOKENS / RPC_URL / DATABASE_URL / HELIUS_API_KEY, psql, python3-websockets.

    python3 scripts/e2e.py            # full run (~10 min)
    python3 scripts/e2e.py --skip-cargo-test
"""
import argparse, asyncio, json, os, re, signal, subprocess, sys, time, urllib.error, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "debug"
LOGS = ROOT / "target" / "e2e"
IDX_HTTP, API_HTTP, API_METRICS = "127.0.0.1:19100", "127.0.0.1:18080", "127.0.0.1:19101"
API_KEY = "e2e-key"
RESULTS = []


def load_env():
    env = dict(os.environ)
    for line in (ROOT / ".env").read_text().splitlines():
        m = re.match(r"^([A-Z_][A-Z0-9_]*)=(.*)$", line.strip())
        if m:
            env.setdefault(m[1], m[2])
    base = env["DATABASE_URL"].split("?")[0]
    env["DATABASE_URL"] = base + "?options=-c%20search_path%3De2e"
    env["E2E_PSQL_URL"] = base + "?options=-csearch_path%3De2e"
    return env


ENV = load_env()


def redact(s):
    return re.sub(r"(api[-_]key=)[^&\s\"']+", r"\1REDACTED", s)


def check(name):
    def deco(fn):
        def run(*a, **kw):
            t0 = time.time()
            try:
                detail = fn(*a, **kw) or ""
                RESULTS.append(("PASS", name, time.time() - t0, str(detail)))
                print(f"  PASS  {name}  ({time.time()-t0:.1f}s) {detail}", flush=True)
                return True
            except Exception as e:  # noqa: BLE001 - report every failure and keep going
                RESULTS.append(("FAIL", name, time.time() - t0, redact(str(e))))
                print(f"  FAIL  {name}: {redact(str(e))}", flush=True)
                return False
        return run
    return deco


def sql(q):
    out = subprocess.run(["psql", "-At", "-v", "ON_ERROR_STOP=1", ENV["E2E_PSQL_URL"], "-c", q],
                         capture_output=True, text=True)
    if out.returncode != 0:
        raise RuntimeError(f"psql: {out.stderr.strip()}")
    return out.stdout.strip()


def sql1(q):
    return sql(q).splitlines()[0] if sql(q) else ""


def run_bin(args, extra_env=None, timeout=900):
    env = {**ENV, **(extra_env or {})}
    return subprocess.run([str(BIN / args[0]), *args[1:]], env=env, capture_output=True, text=True, timeout=timeout)


def start(name, args, extra_env):
    log = open(LOGS / f"{name}.log", "w")
    return subprocess.Popen([str(BIN / args[0]), *args[1:]], env={**ENV, **extra_env}, stdout=log,
                            stderr=subprocess.STDOUT, start_new_session=True)


def http(path, key=API_KEY, base=API_HTTP, raw=False):
    req = urllib.request.Request(f"http://{base}{path}")
    if key:
        req.add_header("x-api-key", key)
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            body = r.read().decode()
            return r.status, (body if raw else json.loads(body))
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        try:
            return e.code, json.loads(body)
        except ValueError:
            return e.code, body


def wait_for(cond, timeout, what, interval=1.0):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            v = cond()
            if v:
                return v
        except Exception:  # noqa: BLE001 - not ready yet
            pass
        time.sleep(interval)
    raise TimeoutError(f"timed out after {timeout}s waiting for {what}")


def stop(proc, timeout=40):
    proc.send_signal(signal.SIGTERM)
    return proc.wait(timeout=timeout)


def log_text(name):
    return re.sub(r"\x1b\[[0-9;]*m", "", (LOGS / f"{name}.log").read_text())


IDX_ENV = {"HTTP_ADDR": IDX_HTTP, "AUDIT_INTERVAL_SECS": "10", "RECONCILE_INTERVAL_SECS": "15",
           "RECONCILE_SAMPLE": "20", "STATS_INTERVAL_SECS": "10", "METADATA_INTERVAL_SECS": "5"}
API_ENV = {"API_ADDR": API_HTTP, "API_METRICS_ADDR": API_METRICS, "API_KEYS": API_KEY,
           "API_RATE_LIMIT_RPS": "100"}

HOLE_SQL = """WITH chain AS (SELECT slot, parent_slot, lag(slot) OVER (ORDER BY slot) prev FROM slots
              WHERE slot <= (SELECT slot FROM indexer_state WHERE key='checkpoint') - 32)
              SELECT count(*) FROM chain WHERE prev IS NOT NULL AND parent_slot > prev"""


# ------------------------------------------------------------------ checks

@check("cargo test (decoder golden tests, stream replay, RPC conversion)")
def t_cargo():
    for attempt in range(3):
        out = subprocess.run(["cargo", "test", "-j", "2", "--workspace"], cwd=ROOT, capture_output=True, text=True,
                             env={**os.environ, "PATH": f"{Path.home()}/.cargo/bin:{os.environ['PATH']}"})
        if "SIGSEGV" in out.stderr or "signal: 11" in out.stderr:
            continue  # this machine's compiler crashes intermittently; retry
        passed = sum(int(n) for n in re.findall(r"test result: ok\. (\d+) passed", out.stdout))
        if out.returncode != 0:
            raise RuntimeError(out.stdout[-1500:] + out.stderr[-1500:])
        return f"{passed} tests passed"
    raise RuntimeError("compiler kept crashing")


@check("fresh schema + migrations")
def t_migrate():
    subprocess.run(["psql", "-q", ENV["DATABASE_URL"].split("?")[0], "-c",
                    "DROP SCHEMA IF EXISTS e2e CASCADE; CREATE SCHEMA e2e;"], check=True, capture_output=True)
    r = run_bin(["dlmm-indexer", "migrate"])
    assert r.returncode == 0, redact(r.stderr[-800:])
    tables = int(sql1("SELECT count(*) FROM information_schema.tables WHERE table_schema='e2e' AND table_type='BASE TABLE'"))
    views = int(sql1("SELECT count(*) FROM information_schema.views WHERE table_schema='e2e'"))
    assert tables >= 12 and views >= 9, (tables, views)
    return f"{tables} tables, {views} views"


@check("tail smoke test (stream -> decode, no DB)")
def t_tail():
    r = run_bin(["dlmm-indexer", "tail", "--duration-secs", "15"], timeout=90)
    summary = [l for l in r.stderr.splitlines() if l.startswith("summary:")]
    assert summary, redact(r.stderr[-600:])
    m = dict(kv.split("=") for kv in summary[0].split()[1:])
    assert int(m["slots_with_meta"]) > 10 and int(m["decode_failures"]) == 0, summary[0]
    return summary[0]


@check("network guard: records devnet, refuses mainnet RPC")
def t_guard():
    r = run_bin(["dlmm-indexer", "snapshot", "--types", "PresetParameter2"])
    assert r.returncode == 0, redact(r.stderr[-600:])
    assert sql1("SELECT value FROM indexer_meta WHERE key='network'") == "devnet"
    before = sql1("SELECT count(*) FROM accounts")
    mainnet = f"https://mainnet.helius-rpc.com/?api-key={ENV['HELIUS_API_KEY']}"
    r = run_bin(["dlmm-indexer", "snapshot", "--types", "PresetParameter2"], {"RPC_URL": mainnet})
    assert r.returncode != 0 and "database belongs to devnet" in r.stderr, redact(r.stderr[-400:])
    assert sql1("SELECT count(*) FROM accounts") == before
    return "mainnet snapshot refused, nothing written"


PROCS = {}


@check("indexer starts, becomes ready, stream/RPC network check passes")
def t_start_indexer():
    PROCS["idx"] = start("indexer", ["dlmm-indexer", "run"], IDX_ENV)
    wait_for(lambda: http("/readyz", None, IDX_HTTP)[0] == 200, 90, "indexer /readyz")
    assert "stream and RPC are on the same network" in wait_for(
        lambda: "same network" in log_text("indexer") and log_text("indexer"), 30, "network check log")
    return http("/readyz", None, IDX_HTTP)[1]


@check("snapshot: pairs + mint decimals + pool reserves")
def t_snapshot():
    r = run_bin(["dlmm-indexer", "snapshot", "--types", "LbPair"], timeout=1200)
    assert r.returncode == 0, redact(r.stderr[-800:])
    # A sample of positions (and bin arrays) so position endpoints always have data.
    r = run_bin(["dlmm-indexer", "snapshot", "--types", "PositionV2,BinArray", "--limit", "25"], timeout=600)
    assert r.returncode == 0, redact(r.stderr[-800:])
    positions = int(sql1("SELECT count(*) FROM accounts WHERE account_type='PositionV2'"))
    assert positions > 0, "no positions sampled"
    pairs = int(sql1("SELECT count(*) FROM lb_pairs"))
    mints = int(sql1("SELECT count(*) FROM mints"))
    reserves = int(sql1("SELECT count(*) FROM pair_reserves WHERE reserve_x_amount IS NOT NULL AND reserve_y_amount IS NOT NULL"))
    assert pairs > 1000 and mints > 1000 and reserves > pairs * 0.9, (pairs, mints, reserves)
    return f"{pairs} pairs, {mints} mints, reserves for {reserves} pairs, {positions} sampled positions"


@check("live data flows into Postgres (swaps with block time, zero decode failures)")
def t_live_data():
    n = wait_for(lambda: int(sql1("SELECT count(*) FROM events WHERE name='Swap'")) >= 3 and
                 int(sql1("SELECT count(*) FROM events WHERE name='Swap'")), 240, ">=3 swaps")
    no_time = int(sql1("SELECT count(*) FROM events WHERE block_time IS NULL"))
    fails = int(sql1("SELECT count(*) FROM decode_failures"))
    assert no_time == 0 and fails == 0, (no_time, fails)
    return f"{n} swaps, {sql1('SELECT count(*) FROM slots')} slots"


@check("swaps view: one row per swap, fee breakdown consistent")
def t_swaps_view():
    ev, view = sql1("SELECT count(*) FROM events WHERE name='Swap'"), sql1("SELECT count(*) FROM swaps")
    bad = sql1("SELECT count(*) FROM swaps WHERE mm_fee IS NOT NULL AND fee <> mm_fee + protocol_fee + coalesce(limit_order_fee,0)")
    assert int(view) >= int(ev) - 5 and int(bad) == 0, (ev, view, bad)  # rows may land between queries
    return f"{view} rows"


@check("background jobs: 24h stats refreshed, token metadata looked up")
def t_jobs():
    wait_for(lambda: int(sql1("SELECT count(*) FROM pair_stats_24h")) > 0, 60, "pair_stats_24h refresh", interval=2)
    wait_for(lambda: sql1("SELECT count(*) FROM mints WHERE metadata_checked_at IS NULL") == "0", 240,
             "token metadata backlog", interval=3)
    stats = sql1("SELECT count(*) || ' pools with 24h stats' FROM pair_stats_24h")
    meta = sql1("SELECT count(symbol) || ' of ' || count(*) || ' mints have a symbol' FROM mints")
    trades = int(sql1("SELECT trades FROM global_stats_24h"))
    assert trades > 0, "global stats empty"
    return f"{stats}; {meta}; {trades} swaps in 24h"


@check("API starts")
def t_start_api():
    PROCS["api"] = start("api", ["dlmm-api"], API_ENV)
    wait_for(lambda: http("/v1/health", None, raw=True)[0] == 200, 30, "api health")
    return http("/v1/status")[1]


def pick(q):
    v = sql1(q)
    assert v, f"no data for: {q}"
    return v


@check("REST: every endpoint returns correct shapes")
def t_rest():
    pair = pick("SELECT lb_pair FROM events WHERE name='Swap' GROUP BY 1 ORDER BY count(*) DESC LIMIT 1")
    wallet = pick(f"SELECT wallet FROM events WHERE name='Swap' AND lb_pair='{pair}' LIMIT 1")
    sig = pick(f"SELECT signature FROM events WHERE name='Swap' AND lb_pair='{pair}' LIMIT 1")
    pos = pick("SELECT pubkey FROM accounts WHERE account_type='PositionV2' AND closed_slot IS NULL LIMIT 1")
    owner = pick(f"SELECT owner_wallet FROM accounts WHERE pubkey='{pos}'")
    expect = {
        "/v1/status": ["checkpoint_slot", "finalized_slot", "seconds_behind"],
        "/v1/stats": ["network", "pairs", "trades_24h", "traders_24h", "active_pairs_24h"],
        "/v1/pairs?limit=3": ["address", "price", "reserve_x_amount", "tvl_in_y", "symbol_x", "trades_24h",
                              "volume_24h_y", "fees_24h_y", "price_change_24h", "logo_x"],
        "/v1/pairs?sort=tvl&limit=3": ["address", "tvl_in_y"],
        "/v1/pairs?sort=volume&limit=3": ["address", "volume_24h_y"],
        "/v1/pairs?sort=change&limit=3": ["address", "price_change_24h"],
        "/v1/pairs?q=SOL&limit=3": ["address", "symbol_x", "symbol_y"],
        f"/v1/pairs?q={pair}": ["address"],
        f"/v1/pairs/{pair}": ["price", "stats_24h", "reserve_x_amount", "tvl_in_y", "decimals_x", "trades_24h"],
        f"/v1/pairs/{pair}/bins?radius=10": ["active_id", "bins"],
        f"/v1/pairs/{pair}/swaps?limit=5": ["signature", "amount_in", "price", "cursor", "decimals_x", "token_x_mint"],
        f"/v1/pairs/{pair}/events?names=Swap2Evt&limit=2": ["name", "data"],
        f"/v1/pairs/{pair}/candles?interval=1m": ["candles", "decimals_adjusted"],
        f"/v1/wallets/{wallet}/swaps?limit=2": ["signature", "trader"],
        f"/v1/wallets/{wallet}/events?limit=2": ["name"],
        f"/v1/wallets/{owner}/positions": ["address", "amount_x", "amount_y", "active_id", "bin_step", "symbol_x"],
        f"/v1/positions/{pos}": ["owner", "amount_x", "events"],
        f"/v1/tx/{sig}": ["instructions", "events", "token_balances"],
    }
    for path, keys in expect.items():
        code, body = http(path)
        assert code == 200, (path, code, body)
        item = body[0] if isinstance(body, list) else body
        assert item, f"{path} returned empty"
        missing = [k for k in keys if k not in item]
        assert not missing, (path, missing)
    top = http("/v1/pairs?sort=tvl&limit=5")[1]
    tvls = [p["tvl_in_y"] for p in top if p["tvl_in_y"] is not None]
    assert tvls == sorted(tvls, reverse=True), "tvl sort"
    active = http("/v1/pairs?sort=trades&limit=5")[1]
    trades = [p["trades_24h"] for p in active]
    assert trades == sorted(trades, reverse=True) and trades[0] > 0, f"trades sort {trades}"
    exact = http(f"/v1/pairs?q={pair}")[1]
    assert len(exact) == 1 and exact[0]["address"] == pair, "address search"
    sol = http("/v1/pairs?q=SOL&limit=20")[1]
    assert sol and all("sol" in ((p["symbol_x"] or "") + (p["symbol_y"] or "")).lower() for p in sol), "symbol search"
    return f"{len(expect)} endpoints; sorts and search verified"


@check("REST: pagination, validation, 404, auth, rate limit")
def t_rest_edges():
    pair = pick("SELECT lb_pair FROM events WHERE name='Swap' GROUP BY 1 ORDER BY count(*) DESC LIMIT 1")
    a = http(f"/v1/pairs/{pair}/swaps?limit=2")[1]
    if len(a) == 2:
        b = http(f"/v1/pairs/{pair}/swaps?limit=2&cursor={a[-1]['cursor']}")[1]
        assert not {x["cursor"] for x in a} & {x["cursor"] for x in b}, "pages overlap"
    assert http("/v1/pairs/not-base58!")[0] == 400
    assert http(f"/v1/pairs/{pair}/candles?interval=7m")[0] == 400
    assert http(f"/v1/pairs/{pair}/swaps?cursor=garbage")[0] == 400
    assert http("/v1/pairs/11111111111111111111111111111111")[0] == 404
    assert http("/v1/status", key=None)[0] == 401
    assert http("/v1/status", key="wrong")[0] == 401
    assert http("/v1/health", key=None, raw=True) == (200, "ok")
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(32) as ex:  # faster than the 100 rps refill, so the burst of 200 runs out
        codes = list(ex.map(lambda _: http("/v1/pairs?limit=1")[0], range(400)))
    assert 429 in codes, f"no rate limiting observed: {set(codes)}"
    time.sleep(3)
    assert http("/v1/status")[0] == 200, "limiter did not refill"
    return f"{codes.count(429)} of 400 concurrent requests limited, then recovered"


@check("WebSocket: live swap pushed within seconds and matches REST")
def t_ws():
    import websockets

    async def go():
        async with websockets.connect(f"ws://{API_HTTP}/v1/ws?api_key={API_KEY}") as ws:
            await ws.send(json.dumps({"op": "subscribe", "channel": "swaps"}))
            await ws.send(json.dumps({"op": "subscribe", "channel": "pairs"}))
            await ws.send(json.dumps({"op": "ping"}))
            seen, t0 = {}, time.time()
            while time.time() - t0 < 240:
                m = json.loads(await asyncio.wait_for(ws.recv(), 240))
                seen[m["type"]] = seen.get(m["type"], 0) + 1
                if m["type"] == "swap":
                    return m["data"], time.time() - m["data"]["block_time"], seen
        raise TimeoutError(f"no swap pushed; got {seen}")

    swap, lag, seen = asyncio.run(go())
    assert seen.get("subscribed") == 2 and seen.get("pong") == 1, seen
    code, tx = http(f"/v1/tx/{swap['signature']}")
    assert code == 200, code
    ev = [e for e in tx["events"] if e["name"] == "Swap" and e["inner_index"] == swap["inner_index"]]
    assert ev and str(ev[0]["data"]["amount_in"]) == swap["amount_in"], "pushed swap differs from REST"
    assert lag < 10, f"push lag {lag:.1f}s"
    return f"block_time->push {lag:.2f}s (block_time has 1s resolution)"


@check("restart: graceful stop, resume from checkpoint, no holes")
def t_restart():
    code = stop(PROCS["idx"])
    assert code == 0 and "stopped cleanly" in log_text("indexer"), f"exit {code}"
    cp = int(sql1("SELECT slot FROM indexer_state WHERE key='checkpoint'"))
    time.sleep(20)
    PROCS["idx"] = start("indexer2", ["dlmm-indexer", "run"], IDX_ENV)
    wait_for(lambda: int(sql1("SELECT slot FROM indexer_state WHERE key='checkpoint'")) > cp + 150, 180, "catch-up")
    assert "resuming from checkpoint" in log_text("indexer2")
    # Slots emitted without block meta leave harmless holes that the audit job repairs;
    # allow it one cycle, then the chain must be contiguous.
    wait_for(lambda: sql1(HOLE_SQL) == "0", 90, "contiguous slot chain", interval=3)
    gaps = sql1("SELECT count(*) || ' recorded / ' || count(repaired_at) || ' repaired' FROM gaps")
    return f"resumed from {cp}, chain contiguous (gaps: {gaps})"


FP = """SELECT md5(coalesce(string_agg(x, '|' ORDER BY x), '')) FROM (
   SELECT concat_ws(',', 'e', slot, signature, ix_index, inner_index, tx_index, name, lb_pair, wallet, position,
                    extract(epoch FROM block_time), data::text) x FROM events WHERE slot BETWEEN {a} AND {b}
   UNION ALL SELECT concat_ws(',', 'i', slot, signature, ix_index, inner_index, tx_index, name, stack_height,
                    invoked_by, accounts::text, args::text, remaining_accounts::text) FROM instructions WHERE slot BETWEEN {a} AND {b}
   UNION ALL SELECT concat_ws(',', 't', slot, signature, tx_index, fee_payer, success, fee, compute_units,
                    account_keys::text, post_token_balances::text, extract(epoch FROM block_time)) FROM transactions WHERE slot BETWEEN {a} AND {b}
   UNION ALL SELECT concat_ws(',', 's', slot, parent_slot, blockhash, block_height, extract(epoch FROM block_time))
                    FROM slots WHERE slot BETWEEN {a} AND {b}) t"""


@check("self-healing: deleted slot range is detected and restored exactly")
def t_gap_repair():
    # A 40-slot window, old enough to be settled, that contains DLMM transactions.
    row = pick("""SELECT slot FROM transactions
                  WHERE slot < (SELECT slot FROM indexer_state WHERE key='checkpoint') - 300
                  ORDER BY slot DESC LIMIT 1""")
    a, b = int(row) - 20, int(row) + 20
    before = sql1(FP.format(a=a, b=b))
    counts = sql1(f"SELECT (SELECT count(*) FROM transactions WHERE slot BETWEEN {a} AND {b}) || '/' || "
                  f"(SELECT count(*) FROM events WHERE slot BETWEEN {a} AND {b})")
    sql(f"""DELETE FROM events WHERE slot BETWEEN {a} AND {b};
            DELETE FROM instructions WHERE slot BETWEEN {a} AND {b};
            DELETE FROM transactions WHERE slot BETWEEN {a} AND {b};
            DELETE FROM slots WHERE slot BETWEEN {a} AND {b};""")
    wait_for(lambda: sql1(f"SELECT count(*) FROM gaps WHERE from_slot <= {a} AND to_slot >= {b} "
                          f"AND repaired_at IS NOT NULL") == "1", 180, "auto audit + repair", interval=2)
    after = sql1(FP.format(a=a, b=b))
    assert before == after, f"restored data differs (before {before}, after {after})"
    return f"slots {a}..{b} (tx/events {counts}) restored byte-identical"


@check("reconciliation: corrupted account is detected and healed from chain")
def t_reconcile():
    # An idle pair (snapshot-only), so a live update can't "heal" it before reconcile does.
    pk = pick("SELECT pubkey FROM accounts WHERE account_type='LbPair' AND closed_slot IS NULL AND write_version = 0 ORDER BY slot LIMIT 1")
    real = sql1(f"SELECT data->>'active_id' FROM accounts WHERE pubkey='{pk}'")
    sql(f"""UPDATE accounts SET data = jsonb_set(data, '{{active_id}}', '123456789'), slot = slot - 5
            WHERE pubkey = '{pk}'""")
    r = run_bin(["dlmm-indexer", "reconcile", "--pubkeys", pk], timeout=120)
    assert r.returncode == 0, redact(r.stderr[-600:])
    report = json.loads(r.stdout.strip().splitlines()[-1])
    now = sql1(f"SELECT data->>'active_id' FROM accounts WHERE pubkey='{pk}'")
    assert report["healed_stale"] == 1 and now != "123456789", (report, now)
    wait_for(lambda: "reconciliation pass" in log_text("indexer2"), 60, "background reconcile pass")
    return f"healed ({report}); active_id {real} -> {now}; background pass ran"


# `up` is generated by Prometheus; rpc_errors_total is labelled by RPC method on first error.
EVENT_ONLY = {"up", "rpc_errors_total"}


@check("metrics: every metric used by dashboard + alerts is exported")
def t_metrics():
    _, idx = http("/metrics", None, IDX_HTTP, raw=True)
    _, api = http("/metrics", None, API_METRICS, raw=True)
    exported = {l.split("{")[0].split(" ")[0] for l in (idx + api).splitlines() if l and not l.startswith("#")}
    exported |= {re.sub(r"_(bucket|sum|count)$", "", m) for m in exported}
    exprs = [t["expr"] for p in json.load(open(ROOT / "deploy/grafana/dashboards/dlmm.json"))["panels"] for t in p["targets"]]
    exprs += re.findall(r"expr:\s*(.+)", (ROOT / "deploy/alerts.yml").read_text())
    funcs = {"rate", "increase", "sum", "by", "le", "histogram_quantile", "name", "route", "status", "job"}
    used = set()
    for e in exprs:
        for tok in re.findall(r"[a-z_][a-z0-9_]*", re.sub(r"\{[^}]*\}|\[[^\]]*\]", "", e)):
            if tok not in funcs and "_" in tok:
                used.add(re.sub(r"_(bucket|sum|count)$", "", tok))
    missing = sorted(m for m in used if m not in exported and m not in EVENT_ONLY)
    assert not missing, f"not exported: {missing}"
    lag = float(re.search(r"^slot_lag (\S+)", idx, re.M)[1])
    assert lag < 150, f"slot_lag {lag}"
    return f"{len(used)} referenced metrics OK; slot_lag={lag:.0f}"


WEB = ROOT / "web"


@check("frontend: typecheck, unit tests, production build, served app talks to the API")
def t_frontend():
    npm = {**os.environ, "PATH": os.environ["PATH"]}
    for cmd in (["npm", "run", "typecheck"], ["npx", "vitest", "run"], ["npm", "run", "build"]):
        r = subprocess.run(cmd, cwd=WEB, capture_output=True, text=True, env=npm, timeout=600)
        assert r.returncode == 0, f"{' '.join(cmd)} failed:\n{r.stdout[-1200:]}{r.stderr[-800:]}"
    # Serve the built bundle against the e2e API, exactly as the dev proxy does.
    log = open(LOGS / "web-preview.log", "w")
    proc = subprocess.Popen(["npx", "vite", "preview", "--port", "14173", "--strictPort"], cwd=WEB, stdout=log,
                            stderr=subprocess.STDOUT, env={**npm, "BINSCOPE_API": f"http://{API_HTTP}"}, start_new_session=True)
    try:
        html = wait_for(lambda: http("/", None, "127.0.0.1:14173", raw=True)[1], 30, "vite preview")
        assert '<div id="root">' in html and "/assets/" in html, "index.html not served"
        # SPA deep links must fall back to index.html.
        code, deep = http("/pool/11111111111111111111111111111111", None, "127.0.0.1:14173", raw=True)
        assert code == 200 and '<div id="root">' in deep, "deep link not served"
        code, stats = http("/v1/stats", API_KEY, "127.0.0.1:14173")
        assert code == 200 and stats["pairs"] > 0, f"proxy to API failed: {code}"
        js = re.findall(r'src="(/assets/[^"]+\.js)"', html)
        assert js and http(js[0], None, "127.0.0.1:14173", raw=True)[0] == 200, "bundle not served"
    finally:
        os.killpg(proc.pid, signal.SIGTERM)
    return "typecheck + unit tests + build OK; bundle, deep links and API proxy served"


@check("graceful shutdown of API and indexer")
def t_shutdown():
    a = stop(PROCS.pop("api"))
    i = stop(PROCS.pop("idx"))
    assert a == 0 and i == 0 and "stopped cleanly" in log_text("indexer2"), (a, i)
    return "both exited 0"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-cargo-test", action="store_true")
    args = ap.parse_args()
    LOGS.mkdir(parents=True, exist_ok=True)
    print(f"E2E against {ENV['GRPC_ENDPOINTS'].split('//')[-1]} — schema e2e, logs in {LOGS}\n", flush=True)
    steps = [t_migrate, t_tail, t_guard, t_start_indexer, t_snapshot, t_live_data, t_swaps_view, t_jobs,
             t_start_api, t_rest, t_rest_edges, t_ws, t_frontend, t_restart, t_gap_repair, t_reconcile, t_metrics,
             t_shutdown]
    if not args.skip_cargo_test:
        steps.insert(0, t_cargo)
    try:
        for step in steps:
            step()
    finally:
        for p in PROCS.values():
            p.send_signal(signal.SIGTERM)
    failed = [r for r in RESULTS if r[0] == "FAIL"]
    print(f"\n{len(RESULTS) - len(failed)}/{len(RESULTS)} checks passed")
    (LOGS / "report.json").write_text(json.dumps(
        [{"status": s, "check": n, "seconds": round(d, 1), "detail": x} for s, n, d, x in RESULTS], indent=1))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
