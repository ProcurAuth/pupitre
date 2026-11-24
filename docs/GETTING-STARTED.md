# Getting started

This guide runs Pupitre on one machine in about ten minutes. It uses the example in `examples/local`. The example has synthetic schemas and playbooks. Nothing in it is a real service.

## What you need

- A Linux machine with the architecture amd64 or arm64.
- `ansible-playbook` of ansible-core 2.19 or later on this machine. It is the program that Pupitre starts for each run. Pupitre checks its version at start and stops with an older version. The key `runner.ansible_playbook` holds its absolute path (the example uses `/usr/bin/ansible-playbook`).
- One test host with Python 3 that accepts an SSH login of the account `deploy` with a key. The playbooks of the example write two files in `/tmp/pupitre-example` on this host.
- `curl`, `tar`, `sha256sum` and `openssl`.

## 1. Download Pupitre

Download the archive and the checksum file. Check the archive, then unpack it.

```sh
curl -fsSLO https://github.com/ProcurAuth/pupitre/releases/download/v0.1.0/pupitre_0.1.0_linux_amd64.tar.gz
curl -fsSLO https://github.com/ProcurAuth/pupitre/releases/download/v0.1.0/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf pupitre_0.1.0_linux_amd64.tar.gz
pupitre_0.1.0_linux_amd64/pupitre --version
```

The last command prints `pupitre 0.1.0` and the licence. The binary is static. It needs no library. To build it from the source, read [DEVELOPMENT.md](DEVELOPMENT.md).

## 2. Make the folder of the example

The example config file has the root `/opt/pupitre-example`. Copy the folder of the example there. To use another root, change the path in each line of `pupitre.hcl`.

```sh
git clone --depth 1 https://github.com/ProcurAuth/pupitre.git
sudo mkdir -p /opt/pupitre-example
sudo chown "$USER" /opt/pupitre-example
cp -R pupitre/examples/local/. /opt/pupitre-example/
mkdir -p /opt/pupitre-example/ssh /dev/shm/pupitre-example
chmod 0700 /dev/shm/pupitre-example
```

The folder `/dev/shm` is a `tmpfs`. The runner writes the secrets of a run there, and it removes them at the end of the run. A folder on a disk stops the start.

## 3. Make the keys

The store needs a key of 32 random bytes in 64 hexadecimal characters. The configuration names the file, never the value. Pupitre derives the key of the secret cipher and the key of the fingerprints from it.

```sh
openssl rand -hex 32 > /opt/pupitre-example/store-key
chmod 0600 /opt/pupitre-example/store-key
```

The runner needs an SSH key and the host keys of the hosts. Use a key that only the test hosts accept. Change the addresses in the last two commands to the addresses of your test hosts.

```sh
ssh-keygen -t ed25519 -N "" -f /opt/pupitre-example/ssh/id_ed25519
ssh-keyscan -t ed25519 203.0.113.10 | awk '{print "production.host-a", $2, $3}' > /opt/pupitre-example/ssh/known_hosts
ssh-keyscan -t ed25519 203.0.113.11 | awk '{print "production.host-b", $2, $3}' >> /opt/pupitre-example/ssh/known_hosts
```

Add the public key `id_ed25519.pub` to the account `deploy` on the test hosts. Pupitre finds the key of a host by the name `<environment>.<host>`, not by its address (4.12 of the [specification](SPECIFICATION.md)). Step 5 makes the hosts `host-a` and `host-b` in the environment `production`, so each line starts with `production.host-a` or `production.host-b`. Pupitre refuses a host that has no line in `known_hosts` (7.8). If one machine is the two test hosts, scan its address two times.

## 4. Check the configuration

The command `check-config` runs the three stages of the checks. It sends nothing on the network. It prints each error with the key, the expected type and the value.

```sh
export PATH="$PWD/pupitre_0.1.0_linux_amd64:$PATH"
pupitre check-config --config /opt/pupitre-example/pupitre.hcl
pupitre print-config --config /opt/pupitre-example/pupitre.hcl
```

The second command prints the value of each key and its source: `file`, `default`, `env:VAR` or `file:/path`. A secret shows as `<hidden>`. Try a mistake: change `max_hosts = 10` to `"10"` and run the first command again. Pupitre stops, and the message names the key and the expected type. Remove the mistake.

The command `check-config` does not check the tmpfs folder, the SSH key file or the known hosts file. The command `serve` checks them at start.

## 5. Make the store and load the inventory

The command `migrate` makes the store. The command `import` reads the output of `ansible-inventory --list --export` and makes the documents of a new environment. Without `--write`, it only prints a report.

```sh
pupitre migrate --config /opt/pupitre-example/pupitre.hcl
pupitre import --config /opt/pupitre-example/pupitre.hcl \
  --inventory-json /opt/pupitre-example/inventory.json --environment production
pupitre import --config /opt/pupitre-example/pupitre.hcl \
  --inventory-json /opt/pupitre-example/inventory.json --environment production --write
```

The file `inventory.json` has two hosts and two groups: `canary` and `relay_hosts`. The schema `relay` names this group, so the import makes the unit `relay`. Change the addresses in the file to the addresses of your test hosts before you run the third command.

## 6. Start the server

```sh
pupitre serve --config /opt/pupitre-example/pupitre.hcl
```

The log goes to the standard error, one JSON object on each line. Open `http://127.0.0.1:8080/` in a browser. The configuration has the mode `open`, so the page asks for no login. A reverse proxy guards the page in a real deployment (see [ACCESS.md](ACCESS.md)). Pupitre writes the warnings `access.mode_open` and `runner.apply_in_open_mode` at each start.

## 7. Change a value and start a run

1. Open the environment `production`, then the unit `relay`. The page shows each key of the schema with its value and its layer.
2. Change `limits.max_clients` on the unit. Save the change. The store keeps the old version in the history.
3. Open the page of the run of the unit. Choose the hosts. Start a check run. The runner builds a snapshot, runs the render playbook, then runs the deploy playbook with `--check --diff`.
4. Read the result of the check. When it has the status `passed`, start the apply run. It needs the check run of the same snapshot hash.
5. Start the status run. It shows the state of each host.
6. Open the commands of the environment. Launch `restart-relay` with a grace period. The command has `check = false`, so the page asks you to confirm.

The example sets `runner.allow_apply_in_open_mode = true`. Without this key, the mode `open` refuses each apply and each command. A check run and a status run still work.

The playbooks write `/tmp/pupitre-example/relay.conf` on the test hosts. After the apply, read this file on a host. It holds the value that you saved.

## 8. Stop and clean

Press Ctrl+C in the terminal of Pupitre. The server stops after the grace time of `server.shutdown_grace`. To remove the example, delete these folders and files:

```sh
rm -rf /opt/pupitre-example /dev/shm/pupitre-example
```

On the test host, remove the folder `/tmp/pupitre-example`.

## Next steps

- [Configuration](CONFIGURATION.md): the rules of the file, the blocks and the commands.
- [Architecture](ARCHITECTURE.md): the layers, the snapshot and the guard of an apply.
- [Access](ACCESS.md): the modes `open` and `authzen`.
- [Operations](OPERATIONS.md): the service, the probes, the backup and the upgrade.
