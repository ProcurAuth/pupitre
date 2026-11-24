# SPDX-License-Identifier: MIT OR Apache-2.0
"""The scenario of the large fixture (8.12): the catalog loads 1,000 units, 1,000 hosts and 65 environments.

The fixture comes from the seeded generator `tests/fixtures/generate.py`. The harness copies its documents into the
folder of the backend `files` before the start of `serve`.
"""
import re
import time
import urllib.request

# The load of the catalog must end within this time. The bound is wide: it finds a defect of the order of a minute.
READY_SECONDS = 120.0


def catalog_load(ctx):
    """2.41, 2.42, 5.6, 5.20: the catalog loads the large fixture, and the lists and the metrics show it."""
    lab, inst, client = ctx.lab, ctx.inst, ctx.client
    counts = lab.fixture_counts
    ready_after = ctx.ready_after
    assert ready_after < READY_SECONDS, "the load of the catalog took %.1fs, the bound is %.0fs" % (ready_after, READY_SECONDS)

    session = client.call("GET", "/api/session").json()
    assert session["runner"] == {"allow_apply": False, "enabled": False}, session

    t0 = time.monotonic()
    environments = client.list_all("/api/environments")
    t_envs = time.monotonic() - t0
    ids = sorted(item["id"] for item in environments)
    expected = sorted(["large"] + ["env-%02d" % n for n in range(64)])
    assert ids == expected, "5.8: the 65 environments of the fixture: %s" % ids[:5]
    assert all("errors" not in item for item in environments), "a document of the fixture has errors: %s" % [i for i in environments if "errors" in i][:2]
    assert counts["environments"] == 65

    t0 = time.monotonic()
    units = client.list_all("/api/environments/large/units")
    t_units = time.monotonic() - t0
    assert len(units) == 1000, len(units)
    assert all(len(item["hosts"]) == 100 for item in units), "each unit has 100 hosts"
    assert all("errors" not in item for item in units), [i for i in units if "errors" in i][:2]
    hosts = client.list_all("/api/environments/large/hosts")
    assert len(hosts) == 1000, len(hosts)
    small_units = client.list_all("/api/environments/env-00/units")
    assert len(small_units) == 2, small_units

    # One effective view: the unit has the schema `s000`, and the host is in the unit (5.12).
    unit = next(item for item in units if item["id"] == "s000-u00")
    host = unit["hosts"][0]
    view = client.call("GET", "/api/environments/large/units/s000-u00/effective?host=%s" % host).json()
    assert view["schema"] == "s000" and view["host"] == host and [k["path"] for k in view["keys"]] == ["banner", "limits.max_clients", "log.level"], view
    # A document of the fixture has one version of the kind `import` (2.18).
    history = client.call("GET", "/api/environments/large/hosts/host-0000/history").json()
    assert [(i["version"], i["kind"]) for i in history["items"]] == [(1, "import")], history

    # The metrics of the ops listener count the documents of the catalog (6.28).
    metrics = urllib.request.urlopen(inst.ops_url() + "/metrics", timeout=10).read().decode()
    found = {}
    for line in metrics.splitlines():
        match = re.match(r'^pupitre_catalog_documents\{kind="([a-z_]+)"\}\s+(\d+)', line)
        if match:
            found[match.group(1)] = int(match.group(2))
    for kind, expected_count in (("environment", 65), ("host", counts["hosts"]), ("unit", counts["units"]), ("unit_values", counts["unit_values"]), ("host_values", counts["host_values"]), ("common", 65)):
        assert found.get(kind) == expected_count, "6.28: pupitre_catalog_documents{kind=%s} is %s, expected %d" % (kind, found.get(kind), expected_count)

    # A write on the loaded store works and the catalog shows it at once (2.41).
    document, etag = client.get_document("/api/environments/env-00/common")
    document["values"]["limits.max_clients"] = 777
    client.save_document("/api/environments/env-00/common", document, etag, "acceptance")
    document, _ = client.get_document("/api/environments/env-00/common")
    assert document["values"]["limits.max_clients"] == 777

    ctx.detail = "ready after %.1fs; list of 65 environments %.2fs; list of 1,000 units %.2fs; fixture digest %s" % (ready_after, t_envs, t_units, lab.fixture_digest[:16])


SCENARIOS = [("large_catalog_load", catalog_load)]
