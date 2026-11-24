# Acceptance profile "local": the profile local of 6.16 with the paths of the test containers.
# The backend files, the access mode open, one runner node. The catalog follows in catalog.hcl.
# The mode open with runs needs allow_apply_in_open_mode (6.12): the tests apply.
# check_max_age has its lowest value, 1m, so that a test can wait for check_too_old (4.24).

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

