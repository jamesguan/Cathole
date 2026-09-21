FROM alpine:3.22
RUN apk add --no-cache bash curl jq wrk
COPY report.lua /bench/report.lua
COPY run.sh /bench/run.sh
RUN chmod 0755 /bench/run.sh
ENTRYPOINT ["/bench/run.sh"]

