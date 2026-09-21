#!/usr/bin/env bash
set -euo pipefail

wg-quick up /etc/wireguard/wg0.conf
trap 'wg-quick down /etc/wireguard/wg0.conf' EXIT
if [[ ${ROLE:?ROLE is required} == server ]]; then
  iptables -t nat -A PREROUTING -p tcp --dport 8080 -j DNAT --to-destination 10.77.0.2:3000
  iptables -t nat -A POSTROUTING -o wg0 -p tcp -d 10.77.0.2 --dport 3000 -j MASQUERADE
  iptables -A FORWARD -i eth0 -o wg0 -p tcp -d 10.77.0.2 --dport 3000 -j ACCEPT
  iptables -A FORWARD -i wg0 -o eth0 -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
else
  backend_ip=$(getent ahostsv4 backend | awk 'NR == 1 { print $1 }')
  backend_interface=$(ip route get "$backend_ip" | awk '{ for (i = 1; i <= NF; i++) if ($i == "dev") { print $(i + 1); exit } }')
  iptables -t nat -A PREROUTING -i wg0 -p tcp --dport 3000 -j DNAT --to-destination "$backend_ip:3000"
  iptables -t nat -A POSTROUTING -o "$backend_interface" -p tcp -d "$backend_ip" --dport 3000 -j MASQUERADE
  iptables -A FORWARD -i wg0 -o "$backend_interface" -p tcp -d "$backend_ip" --dport 3000 -j ACCEPT
  iptables -A FORWARD -i "$backend_interface" -o wg0 -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
fi

exec tail -f /dev/null
