# Deployment examples

Linux systemd: build the release binary for the target architecture, install it
as `/usr/local/bin/cathole`, create a dedicated `cathole` system user/group,
place the config/identity files in `/etc/cathole`, and install the unit template.
Set files to root:cathole 0640 and the directory to 0750. Enable either
`cathole@server` or `cathole@client`. The service uses no network administration
capability and needs no TUN device. These examples assume ports above 1023.

Docker:

```sh
docker build -t cathole:local .
docker run --rm --name cathole-server \
  -v "$PWD/identity:/config:ro" \
  -p 2333:2333/udp -p 5202:5202/tcp -p 5202:5202/udp \
  cathole:local /config/server.toml
```

Set server bind addresses to `0.0.0.0` inside the container; generated loopback
defaults are deliberately not reachable through published container ports.
Ensure UID 10001 can read only the required mounted files. Client `local_addr`
must refer to an address reachable from its own container; `127.0.0.1` refers to
that container, not the host. Do not publish ports you have not configured.

The Dockerfile and unit are supplied examples; building/running them on Linux
has not been validated on the Windows development host. No infrastructure or
firewall configuration was modified while developing this project.
