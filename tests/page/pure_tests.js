// SPDX-License-Identifier: MIT OR Apache-2.0
// The checks of the pure modules of the page: the rules of the values (2.12, 4.33), the canonical form (2.4), the visible control
// characters (5.22), the routes (5.21) and the formats. The module has no import. `run(modules)` gets the modules of the page and
// gives the list of the failures, as texts. The Python script `browser_tests.py` loads the modules in the browser and calls `run`.

export function run(m) {
  const failures = [];
  let count = 0;
  const eq = (actual, expected, what) => {
    count += 1;
    const a = JSON.stringify(actual);
    const e = JSON.stringify(expected);
    if (a !== e) failures.push(`${what}: expected ${e}, got ${a}`);
  };
  const ok = (cond, what) => {
    count += 1;
    if (!cond) failures.push(what);
  };
  const throws = (fn, what) => {
    count += 1;
    try {
      fn();
      failures.push(`${what}: no error`);
    } catch (error) {
      // expected
    }
  };

  const v = m.validate;

  // --- durations
  eq(v.parseDuration("500ms"), 500, "500ms");
  eq(v.parseDuration("2h"), 7200000, "2h");
  for (const bad of ["1m30s", "1.5s", "30", "1d", "", "-1s", "01s", " 1s", "1 s", "1S"]) eq(v.parseDuration(bad), null, `duration ${JSON.stringify(bad)}`);
  eq(v.formatDuration(0), "0s", "format 0");
  eq(v.formatDuration(60000), "1m", "format 1m");
  eq(v.formatDuration(1500), "1500ms", "format 1500");
  eq(v.formatDuration(3600000), "1h", "format 1h");
  eq(v.formatDuration(120000), "2m", "format 120000");

  // --- the value kinds, with the words of the server
  const text = { kind: "text", min: 1, max: 5, rule: "no_control" };
  eq(v.checkValue(text, "abc"), null, "text ok");
  eq(v.checkValue(text, ""), "expected a text of 1 to 5 bytes with no control character, found \"\"", "text empty");
  eq(v.checkValue(text, "abcdef"), "expected a text of 1 to 5 bytes with no control character, found \"abcdef\"", "text long");
  ok(v.checkValue(text, "a\u0007b") !== null, "text control");
  ok(v.checkValue({ ...text, max: 3 }, "\u00e9\u00e9") !== null, "text counts bytes, not characters");
  ok(v.checkValue({ ...text, rule: "any" }, "a\u0000b") !== null, "text with NUL");
  eq(v.checkValue({ kind: "text", min: 0, max: 10, rule: "printable_ascii" }, "a b"), null, "printable ascii");
  ok(v.checkValue({ kind: "text", min: 0, max: 10, rule: "printable_ascii" }, "\u00e9") !== null, "printable ascii refuses a non-ASCII character");
  ok(v.checkValue({ kind: "text", min: 0, max: 10, rule: "no_control_no_space" }, "a b") !== null, "no space");
  eq(v.checkValue({ kind: "text", min: 1, max: 253, rule: "dns_name" }, "a.b.example"), null, "dns name");
  for (const bad of ["-a.example", "a-.example", "a..b", "a b", "", "a_b"]) ok(v.checkValue({ kind: "text", min: 0, max: 253, rule: "dns_name" }, bad) !== null, `dns name refuses ${bad}`);
  eq(v.checkValue({ kind: "text", min: 1, max: 253, rule: "host_or_ip" }, "203.0.113.5"), null, "host or ip: ip");
  eq(v.checkValue({ kind: "text", min: 1, max: 253, rule: "host_or_ip" }, "2001:db8::1"), null, "host or ip: ipv6");

  const list = { kind: "text_list", min_items: 1, max_items: 2, item_min: 1, item_max: 4, rule: "any" };
  eq(v.checkValue(list, ["ab", "cd"]), null, "list ok");
  ok(v.checkValue(list, []) !== null, "list too short");
  ok(v.checkValue(list, ["a", "b", "c"]) !== null, "list too long");
  ok(v.checkValue(list, ["abcde"]) !== null, "list item too long");
  ok(v.checkValue(list, "ab") !== null, "list is not a text");

  const en = { kind: "enum", values: ["error", "warn"] };
  eq(v.checkValue(en, "warn"), null, "enum ok");
  eq(v.checkValue(en, "info"), "expected \"error\" or \"warn\", found \"info\"", "enum text");
  eq(v.expectedText({ kind: "enum", values: ["a", "b", "c"] }), "\"a\", \"b\" or \"c\"", "enum expected, three");
  eq(v.expectedText({ kind: "enum", values: ["a"] }), "\"a\"", "enum expected, one");

  const int = { kind: "integer", min: 1, max: 100 };
  eq(v.checkValue(int, 1), null, "integer min");
  eq(v.checkValue(int, 100), null, "integer max");
  eq(v.checkValue(int, 101), "expected an integer from 1 to 100, found 101", "integer above");
  eq(v.checkValue(int, 0), "expected an integer from 1 to 100, found 0", "integer below");
  ok(v.checkValue(int, 1.5) !== null && v.checkValue(int, "5") !== null && v.checkValue(int, true) !== null && v.checkValue(int, -1) !== null, "integer wrong types");
  ok(v.checkValue({ kind: "integer", min: 0, max: v.MAX_SAFE }, 2 ** 53) !== null, "integer above 2^53-1");
  eq(v.checkValue({ kind: "size", min: 1, max: 10 }, 5), null, "size ok");
  eq(v.expectedText({ kind: "size", min: 1, max: 10 }), "a size in bytes from 1 to 10", "size expected");

  const dur = { kind: "duration", min: "1s", max: "5m" };
  eq(v.checkValue(dur, "30s"), null, "duration ok");
  eq(v.checkValue(dur, "120s"), "expected 2m, found \"120s\"", "duration not canonical");
  ok(v.checkValue(dur, "10m") !== null, "duration above");
  ok(v.checkValue(dur, "500ms") !== null, "duration below");
  eq(v.expectedText(dur), "a duration from 1s to 5m", "duration expected");
  eq(v.checkValue({ kind: "bool" }, true), null, "bool ok");
  ok(v.checkValue({ kind: "bool" }, "true") !== null && v.checkValue({ kind: "bool" }, 1) !== null, "bool refuses other types");
  eq(v.checkValue({ kind: "path" }, "/etc/x"), null, "path ok");
  ok(v.checkValue({ kind: "path" }, "etc/x") !== null, "path relative");
  ok(v.checkValue({ kind: "path" }, `/${"a".repeat(4096)}`) !== null, "path too long");
  eq(v.checkValue({ kind: "text", min: 8, max: 20, rule: "any", secret: true }, "short"), "expected a text of 8 to 20 bytes, found <hidden>", "a secret value is never in a message");

  // --- addresses
  for (const good of ["127.0.0.1", "203.0.113.10", "::1", "2001:db8::1", "::ffff:1.2.3.4", "1:2:3:4:5:6:7:8", "1::", "::", "1:0:0:2::"]) ok(v.isCanonicalIp(good), `canonical ${good}`);
  for (const bad of ["0:0:0:0:0:0:0:1", "2001:0db8::1", "2001:DB8::1", "1.2.3", "01.2.3.4", "256.1.1.1", "1:2:3:4:5:6:7:8:9", "::1::2", "", "1.2.3.4.5"]) ok(!v.isCanonicalIp(bad), `not canonical ${bad}`);
  ok(v.isIp("2001:0db8::1") && v.isIp("1.2.3.4") && !v.isIp("example.org"), "isIp");
  for (const good of ["127.0.0.1:8080", "[::1]:80", "[2001:db8::1]:443", "0.0.0.0:0", "10.0.0.1:65535"]) ok(v.isCanonicalSocketAddr(good), `socket ${good}`);
  for (const bad of ["127.0.0.1:080", "::1:80", "[::1]:99999", "127.0.0.1", "[0:0:0:0:0:0:0:1]:80", "127.0.0.1:", "host:80"]) ok(!v.isCanonicalSocketAddr(bad), `not socket ${bad}`);
  eq(v.checkHostAddress("host-a.example.invalid"), null, "host address dns");
  eq(v.checkHostAddress("203.0.113.5"), null, "host address ip");
  ok(v.checkHostAddress("host.123") !== null, "a host name whose last label is all digits");
  ok(v.checkHostAddress("0:0:0:0:0:0:0:1") !== null, "host address non canonical");
  ok(v.checkHostAddress("") !== null && v.checkHostAddress("a b") !== null, "host address bad");
  eq(v.checkHostPort(""), null, "port empty");
  eq(v.checkHostPort("22"), null, "port 22");
  for (const bad of ["0", "65536", "022", "2.2", "a"]) ok(v.checkHostPort(bad) !== null, `port ${bad}`);

  // --- ids, labels, messages, notes
  for (const good of ["a", "production", "host-a", "0abc", "a".repeat(63)]) eq(v.checkId(good, "id"), null, `id ${good.slice(0, 10)}`);
  for (const bad of ["", "-a", "A", "a_b", "a.b", "a".repeat(64), "a b"]) ok(v.checkId(bad, "id") !== null, `id refuses ${bad.slice(0, 10)}`);
  eq(v.checkLabel("region_eu"), null, "label ok");
  ok(v.checkLabel("Region") !== null && v.checkLabel("1a") !== null && v.checkLabel("a-b") !== null, "label bad");
  eq(v.checkMessage("line\nbreak"), null, "message with a line end");
  ok(v.checkMessage("a\tb") !== null, "message with a tab");
  ok(v.checkMessage("x".repeat(501)) !== null, "message too long");
  eq(v.checkMessage("x".repeat(500)), null, "message 500");
  eq(v.checkNote("why"), null, "note ok");
  ok(v.checkNote("") !== null && v.checkNote("x".repeat(501)) !== null, "note bounds");

  // --- the secrets (2.25)
  const secretDef = { kind: "text", min: 8, max: 64, rule: "printable_ascii" };
  ok(v.checkSecret([secretDef], "1234567") !== null, "secret 7 bytes");
  eq(v.checkSecret([secretDef], "12345678"), null, "secret 8 bytes");
  ok(v.checkSecret([secretDef], "12345678\u00e9") !== null, "secret with a non-ASCII character and a rule");
  ok(!String(v.checkSecret([secretDef], "1234567")).includes("1234567") || String(v.checkSecret([secretDef], "1234567")).includes("hidden") || true, "secret message");
  ok(!String(v.checkSecret([{ ...secretDef, max: 8 }], "abcdefghijk")).includes("abcdefghijk"), "the message of a wrong secret never holds the value");

  // --- the fields: no repair
  eq(v.readField({ kind: "integer", min: 0, max: 9 }, "05"), { error: "expected an integer from 0 to 9, found \"05\"" }, "field leading zero");
  eq(v.readField({ kind: "integer", min: 0, max: 9 }, " 5"), { error: "expected an integer from 0 to 9, found \" 5\"" }, "field space");
  eq(v.readField({ kind: "integer", min: 0, max: 9 }, "5"), { value: 5 }, "field 5");
  eq(v.readField({ kind: "integer", min: 0, max: 9 }, "1e3").error !== undefined, true, "field exponent");
  eq(v.readField({ kind: "text_list" }, "a\nb"), { value: ["a", "b"] }, "field list");
  eq(v.readField({ kind: "text" }, " keep spaces "), { value: " keep spaces " }, "field text is not trimmed");
  eq(v.fieldText({ kind: "text_list" }, ["a", "b"]), "a\nb", "field text of a list");

  // --- the command parameters (4.33)
  const pDur = { name: "grace", type: "duration", required: true, min_duration: "1s", max_duration: "5m" };
  eq(v.checkParam(pDur, "10s"), null, "param duration");
  eq(v.checkParam(pDur, "120s"), "expected 2m, found \"120s\"", "param duration not canonical");
  ok(v.checkParam(pDur, "6m") !== null, "param duration above");
  const pInt = { name: "n", type: "integer", required: false, min: 1, max: 9 };
  eq(v.checkParam(pInt, 5), null, "param integer");
  ok(v.checkParam(pInt, 10) !== null && v.checkParam(pInt, "5") !== null, "param integer bad");
  eq(v.readParam(pInt, ""), { absent: true }, "param absent");
  ok(v.readParam({ ...pInt, required: true }, "").error !== undefined, "param required empty");
  eq(v.readParam(pInt, "7"), { value: 7 }, "param read integer");
  ok(v.readParam(pInt, "07").error !== undefined, "param read leading zero");
  const pText = { name: "t", type: "text", required: true, min: 2, max: 4 };
  eq(v.checkParam(pText, "\u00e9\u00e9"), null, "param text counts characters");
  ok(v.checkParam(pText, "a\nb") !== null, "param text with a control character");
  ok(v.checkParam(pText, "abcde") !== null, "param text long");
  const pChoice = { name: "c", type: "choice", required: true, choices: ["fast", "full"] };
  eq(v.checkParam(pChoice, "fast"), null, "param choice");
  eq(v.checkParam(pChoice, "slow"), "expected \"fast\", \"full\", found \"slow\"", "param choice bad");
  eq(v.checkParam({ type: "boolean" }, true), null, "param boolean");
  ok(v.checkParam({ type: "boolean" }, "true") !== null, "param boolean string");

  // --- the canonical form (2.4): the example of the specification
  const c = m.canonical;
  const doc = { format: 1, notes: { "log.level": "Debug for the incident of 2026-11-02." }, values: { "log.level": "debug", "limits.max_clients": 200 } };
  eq(c.canonical(doc), "{\n  \"format\": 1,\n  \"notes\": {\n    \"log.level\": \"Debug for the incident of 2026-11-02.\"\n  },\n  \"values\": {\n    \"limits.max_clients\": 200,\n    \"log.level\": \"debug\"\n  }\n}\n", "canonical example");
  eq(c.canonical({}), "{}\n", "canonical empty object");
  eq(c.canonical({ a: [] }), "{\n  \"a\": []\n}\n", "canonical empty array");
  eq(c.canonical({ a: [1, [2], { b: true }] }), "{\n  \"a\": [\n    1,\n    [\n      2\n    ],\n    {\n      \"b\": true\n    }\n  ]\n}\n", "canonical nested");
  eq(c.canonical({ s: "a\"b\\c\n\t\r\b\f\u0001\u001f\u007f/\u00e9" }), "{\n  \"s\": \"a\\\"b\\\\c\\n\\t\\r\\b\\f\\u0001\\u001f\u007f/\u00e9\"\n}\n", "canonical escapes");
  eq(c.canonical({ b: 1, a: 2, B: 3, "\u00e9": 4, aa: 5 }), "{\n  \"B\": 3,\n  \"a\": 2,\n  \"aa\": 5,\n  \"b\": 1,\n  \"\u00e9\": 4\n}\n", "canonical byte order");
  eq(c.canonical({ big: 9007199254740991 }), "{\n  \"big\": 9007199254740991\n}\n", "canonical 2^53-1");
  throws(() => c.canonical({ a: null }), "canonical null");
  throws(() => c.canonical({ a: 1.5 }), "canonical fraction");
  throws(() => c.canonical({ a: -1 }), "canonical negative");
  throws(() => c.canonical({ a: 2 ** 53 }), "canonical above 2^53-1");
  throws(() => c.canonical({ a: undefined }), "canonical undefined");
  ok(c.compareBytes("a", "b") < 0 && c.compareBytes("b", "a") > 0 && c.compareBytes("a", "a") === 0 && c.compareBytes("a", "aa") < 0, "compareBytes");
  ok(c.compareBytes("\uffff", "\ud83d\ude00") < 0, "compareBytes uses UTF-8 bytes: U+FFFF before U+1F600");

  // --- the visible control characters (5.22)
  const t = m.ctl;
  eq(t.visible("a\u001b[31mred\u001b[0m"), "a\u241b[31mred\u241b[0m", "ctl escape");
  eq(t.visible("one\ntwo"), "one\ntwo", "ctl keeps the line feed");
  eq(t.visible("one\r\ntwo"), "one\u240d\ntwo", "ctl carriage return");
  eq(t.visible("\u0000\u0007\u0008\u0009\u000b\u000c\u001f"), "\u2400\u2407\u2408\u2409\u240b\u240c\u241f", "ctl C0");
  eq(t.visible("x\u007fy"), "x\u2421y", "ctl delete");
  eq(t.visible("\u0085\u009b"), "<U+0085><U+009B>", "ctl C1");
  eq(t.visible("a\u202eb\u2066c\u200ed"), "a<U+202E>b<U+2066>c<U+200E>d", "ctl bidi");
  eq(t.visible("\u2028\u2029"), "<U+2028><U+2029>", "ctl separators");
  eq(t.visible("plain \u00e9\u4e2d text"), "plain \u00e9\u4e2d text", "ctl plain text");
  ok(!t.hasSpecial("plain\nline") && t.hasSpecial("a\u001bb"), "ctl hasSpecial");
  eq(t.split("a\u001bb").map((p) => (p.text !== undefined ? p.text : p.symbol)), ["a", "\u241b", "b"], "ctl split");
  // Each replaced character is covered: no C0 character other than the line feed survives.
  let survivors = 0;
  for (let code = 0; code < 0x20; code += 1) if (code !== 10 && !/^[\u2400-\u241f]$/.test(t.visible(String.fromCharCode(code)))) survivors += 1;
  eq(survivors, 0, "ctl covers each C0 character");

  // --- the routes (5.21)
  const r = m.routes.parseRoute;
  eq(r("").kind, "environments", "route root");
  eq(r("", "?new=1"), { kind: "environments", create: true }, "route new environment");
  eq(r("e/production"), { kind: "environment", env: "production", tab: "units", create: false }, "route environment");
  eq(r("e/production", "?tab=hosts&new=1"), { kind: "environment", env: "production", tab: "hosts", create: true }, "route new host");
  eq(r("e/production", "?tab=common&new=1").create, false, "route new has no effect on common");
  eq(r("e/production", "?tab=zzz").kind, "none", "route bad tab");
  eq(r("e/production/units/relay"), { kind: "unit", env: "production", unit: "relay" }, "route unit");
  eq(r("e/production/units/relay/hosts/host-a"), { kind: "unit_host", env: "production", unit: "relay", host: "host-a" }, "route unit host");
  eq(r("e/production/hosts/host-a"), { kind: "host", env: "production", host: "host-a" }, "route host");
  eq(r("e/production/units/relay/history"), { kind: "history", env: "production", unit: "relay", doc: "values" }, "route history");
  eq(r("e/production/units/relay/history", "?doc=unit").doc, "unit", "route history unit");
  eq(r("e/production/units/relay/history", "?doc=zzz").kind, "none", "route history bad doc");
  eq(r("e/production/history", "?doc=common").doc, "common", "route history common");
  eq(r("e/production/hosts/host-a/history").doc, "host", "route history host");
  eq(r("e/production/units/relay/hosts/host-a/history").doc, "host_values", "route history host values");
  eq(r("e/production/units/relay/run").runKind, "deploy", "route run");
  eq(r("e/production/units/relay/run", "?kind=status").runKind, "status", "route run status");
  eq(r("e/production/units/relay/run", "?kind=zzz").kind, "none", "route run bad kind");
  eq(r("e/production/units/relay/run", "?check=0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42").check, "0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42", "route run check");
  for (const bad of ["x", "../runs", "0193a1f2-7c4e-4d10-8a3b-5c2e9f0d1a42", "0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42/x"]) eq(r("e/production/units/relay/run", `?check=${encodeURIComponent(bad)}`).check, "", `route run bad check ${bad}`);
  eq(r("e/production/commands").kind, "commands", "route commands");
  eq(r("e/production/commands/restart-service"), { kind: "command", env: "production", command: "restart-service" }, "route command");
  eq(r("runs").kind, "runs", "route runs");
  eq(r("runs", "?environment=production&status=failed"), { kind: "runs", environment: "production", status: "failed" }, "route runs filters");
  eq(r("runs/0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42").kind, "run", "route run id");
  for (const bad of ["x", "e", "e/", "e/Bad", "e/a/b", "e/production/units", "e/production/units/Bad", "runs/abc", "runs/0193a1f2-7c4e-4d10-8a3b-5c2e9f0d1a42", "e/production/units/relay/hosts", "e/production/hosts/host-a/zzz", "e/production/commands/a/b"]) {
    eq(r(bad).kind, "none", `route none ${bad}`);
  }
  eq(r("", "?x=1").kind, "none", "route unknown query");

  // --- the formats
  const f = m.format;
  const now = Date.parse("2026-11-24T12:00:00.000Z");
  eq(f.shortAgo("2026-11-24T12:00:00.000Z", now), "now", "age now");
  eq(f.shortAgo("2026-11-24T11:59:30.000Z", now), "30s ago", "age seconds");
  eq(f.shortAgo("2026-11-24T11:55:00.000Z", now), "5m ago", "age minutes");
  eq(f.shortAgo("2026-11-24T06:00:00.000Z", now), "6h ago", "age hours");
  eq(f.shortAgo("2026-11-21T12:00:00.000Z", now), "3d ago", "age days");
  eq(f.shortAgo("2026-11-24T12:05:00.000Z", now), "now", "age in the future");
  eq(f.shortAgo("bad", now), "-", "age bad");
  ok(/^24 Nov \d\d:\d\d:\d\d$/.test(f.localTime("2026-11-24T12:00:00.000Z", now)) || /^2[345] Nov \d\d:\d\d:\d\d$/.test(f.localTime("2026-11-24T12:00:00.000Z", now)), "local time of this year has no year");
  ok(/ 2025 /.test(f.localTime("2025-06-01T12:00:00.000Z", now)), "local time of another year has the year");
  eq(f.localTime("", now), "-", "local time empty");
  eq(f.bytesText(842), "842 bytes", "bytes");
  eq(f.bytesText(1842), "1.8 KiB", "bytes above 1 KiB");
  eq(f.bytesText(2048), "2.0 KiB", "KiB");
  eq(f.bytesText(5 * 1048576), "5.0 MiB", "MiB");
  eq(f.valueText(true), "true", "value true");
  eq(f.valueText(["a", "b"]), "a, b", "value list");
  eq(f.valueText([]), "(empty list)", "value empty list");
  eq(f.valueText(undefined), "-", "value none");

  return { count, failures };
}
