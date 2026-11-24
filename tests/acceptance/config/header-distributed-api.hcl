# Acceptance profile "distributed", API instance: the same store as the runner node, runner.execute = false.
# The instance accepts run requests and writes them in the store with the status queued (4.1).
# It has no key of the runner node (6.12).

server {
  listen     = "0.0.0.0:8080"
  ops_listen = "0.0.0.0:9090"
  allowed_hosts = ["pupitre.example.invalid"]
  base_path  = "/"
}

log {
  level = "debug"
}

telemetry {
  enabled = false
}

storage {
  backend         = "postgres"
  secret_key_file = "/run/secrets/pupitre/store-key"

  postgres {
    host          = "postgres.example.invalid"
    port          = 5432
    database      = "pupitre"
    user_file     = "/run/secrets/pg/username"
    password_file = "/run/secrets/pg/password"
    sslmode       = "verify-full"
    poll_interval = "1s"

    tls {
      ca_file     = "/run/secrets/pg-ca/ca.crt"
      server_name = "postgres.example.invalid"
    }
  }
}

access {
  mode = "open"
}

runner {
  enabled                  = true
  execute                  = false
  allow_apply_in_open_mode = true
  check_max_age            = "1m"
  max_hosts                = 100
}

