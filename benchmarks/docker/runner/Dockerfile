FROM alpine:3.22
RUN apk add --no-cache bash curl jq wrk python3
COPY report.lua /bench/report.lua
COPY probe.py /bench/probe.py
COPY run.sh /bench/run.sh
RUN chmod 0755 /bench/run.sh /bench/probe.py
ENTRYPOINT ["/bench/run.sh"]
