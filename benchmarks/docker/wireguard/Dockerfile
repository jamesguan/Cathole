FROM alpine:3.22
RUN apk add --no-cache bash iproute2 iptables wireguard-tools
COPY start.sh /usr/local/bin/start-wireguard
RUN chmod 0755 /usr/local/bin/start-wireguard
ENTRYPOINT ["/usr/local/bin/start-wireguard"]
