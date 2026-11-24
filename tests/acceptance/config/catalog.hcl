# The catalog of capabilities of the acceptance profiles (4.2, 4.32): one schema, the playbooks and the commands.
# The harness adds this text at the end of each header file. The paths are the paths inside the runner container.

schema "demo" {
  path = "/srv/ansible/schemas/demo.json"
}

playbook "deploy-demo" {
  kind = "deploy"
  path = "/srv/ansible/playbooks/deploy-demo.yml"
}

playbook "render-demo" {
  kind = "render"
  path = "/srv/ansible/playbooks/render-demo.yml"
}

playbook "status-demo" {
  kind = "status"
  path = "/srv/ansible/playbooks/status-demo.yml"
}

playbook "restart-service" {
  kind = "command"
  path = "/srv/ansible/playbooks/restart-service.yml"
}

playbook "rotate-banner" {
  kind = "command"
  path = "/srv/ansible/playbooks/rotate-banner.yml"
}

playbook "slow-task" {
  kind = "command"
  path = "/srv/ansible/playbooks/slow-task.yml"
}

playbook "leak-secret" {
  kind = "command"
  path = "/srv/ansible/playbooks/leak-secret.yml"
}

# A command with check = false: it runs after the confirmation of the person.
command "restart-service" {
  label    = "Restart the service"
  playbook = "restart-service"
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

# A command with check = true: an apply needs a check of the same snapshot (4.24).
command "rotate-banner" {
  label    = "Rotate the banner"
  playbook = "rotate-banner"
  target   = "unit"
  check    = true

  param "banner" {
    label    = "New banner"
    type     = "text"
    required = true
    min      = 1
    max      = 64
  }
}

# A command that holds a run open, for the tests of the cancel and of the adoption.
command "slow-task" {
  label    = "Hold the run open"
  playbook = "slow-task"
  target   = "unit"
  check    = false

  param "seconds" {
    label    = "Seconds"
    type     = "integer"
    required = true
    min      = 1
    max      = 300
  }
}

# A command that prints the secrets of the unit on purpose, for the test of the scrubber (4.21).
command "leak-secret" {
  label    = "Print the secrets"
  playbook = "leak-secret"
  target   = "unit"
  check    = false

  param "mode" {
    label    = "Mode"
    type     = "choice"
    required = true
    choices  = ["print", "fail"]
  }
}
