# Acceptance profile "large": the backend files with the large fixture (8.12), and no runs.
# The configuration has 100 schemas: the harness adds the blocks that the generator writes, in place of catalog.hcl.
# The fixture has 65 environments: 1 large environment and 64 small ones. The default limit of 64 is too low (2.42).

server {
  listen     = "0.0.0.0:8080"
  ops_listen = "0.0.0.0:9090"
  allowed_hosts = ["pupitre.example.invalid"]
  base_path  = "/"
}

log {
  level = "info"
}

telemetry {
  enabled = false
}

storage {
  backend         = "files"
  secret_key_file = "/run/secrets/pupitre/store-key"

  files {
    path            = "/var/lib/pupitre/data"
    secrets_path    = "/var/lib/pupitre/secrets"
    runs_path       = "/var/lib/pupitre/runs"
    rescan_interval = "1s"
  }
}

access {
  mode = "open"
}

runner {
  enabled = false
}

limits {
  max_environments = 100
}

