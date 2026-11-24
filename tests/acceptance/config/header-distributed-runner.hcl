# Acceptance profile "distributed", runner node: the shape of 6.17 with the backend postgres and TLS to PostgreSQL.
# The access mode is open, not authzen: the acceptance tests have no PDP and no edge (the series U of L3 tests authzen).
# This instance has runner.execute = true: it holds the lease runner (4.8).

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
  execute                  = true
  allow_apply_in_open_mode = true
  check_max_age            = "1m"
  max_hosts                = 100
  work_dir                 = "/var/lib/pupitre/runner"
  secrets_dir              = "/run/pupitre/secrets"
  ansible_playbook         = "/opt/ansible/bin/ansible-playbook"
  path                     = "/usr/local/bin:/usr/bin:/bin"
  ssh_user                 = "deploy"
  ssh_key_file             = "/run/secrets/ssh/id_ed25519"
  known_hosts_file         = "/run/secrets/ssh/known_hosts"
  run_timeout              = "10m"
  cancel_grace             = "3s"
  lease_ttl                = "10s"
}

