# Example configuration of Pupitre: the profile local, with synthetic schemas and playbooks.
# The root of the example is /opt/pupitre-example. Change this path in each line, or copy the folder there.
# The key of the store is in a file, never in this file. See docs/GETTING-STARTED.md for the steps.

server {
  listen     = "127.0.0.1:8080"
  ops_listen = "127.0.0.1:9090"
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
  secret_key_file = "/opt/pupitre-example/store-key"

  files {
    path         = "/opt/pupitre-example/data"
    secrets_path = "/opt/pupitre-example/secrets"
    runs_path    = "/opt/pupitre-example/runs"
  }
}

access {
  mode = "open"
}

runner {
  enabled                  = true
  execute                  = true
  allow_apply_in_open_mode = true
  check_max_age            = "30m"
  max_hosts                = 10
  work_dir                 = "/opt/pupitre-example/work"
  secrets_dir              = "/dev/shm/pupitre-example"
  ansible_playbook         = "/usr/bin/ansible-playbook"
  path                     = "/usr/local/bin:/usr/bin:/bin"
  ssh_user                 = "deploy"
  ssh_key_file             = "/opt/pupitre-example/ssh/id_ed25519"
  known_hosts_file         = "/opt/pupitre-example/ssh/known_hosts"
  run_timeout              = "30m"
}

schema "relay" {
  path = "/opt/pupitre-example/schemas/relay.json"
}

playbook "deploy-relay" {
  kind = "deploy"
  path = "/opt/pupitre-example/playbooks/deploy-relay.yml"
}

playbook "render-relay" {
  kind = "render"
  path = "/opt/pupitre-example/playbooks/render-relay.yml"
}

playbook "status-relay" {
  kind = "status"
  path = "/opt/pupitre-example/playbooks/status-relay.yml"
}

playbook "restart-relay" {
  kind = "command"
  path = "/opt/pupitre-example/playbooks/restart-relay.yml"
}

command "restart-relay" {
  label    = "Restart the relay"
  playbook = "restart-relay"
  target   = "unit"
  check    = false

  param "grace" {
    label        = "Grace period"
    type         = "duration"
    required     = true
    min_duration = "1s"
    max_duration = "5m"
  }
}
