done = function(summary, latency, requests)
  local seconds = summary.duration / 1000000
  local connect = summary.errors.connect
  local read = summary.errors.read
  local write = summary.errors.write
  local status = summary.errors.status
  local timeout = summary.errors.timeout
  local errors = connect + read + write + status + timeout
  io.write(string.format(
    'WRK_JSON:{"requests":%d,"duration_seconds":%.6f,"bytes":%d,' ..
    '"requests_per_second":%.6f,"bytes_per_second":%.6f,' ..
    '"latency_us_min":%.3f,"latency_us_max":%.3f,"latency_us_mean":%.3f,"latency_us_stdev":%.3f,' ..
    '"latency_us_p50":%.3f,"latency_us_p75":%.3f,"latency_us_p90":%.3f,' ..
    '"latency_us_p95":%.3f,"latency_us_p99":%.3f,' ..
    '"req_per_sec_min":%.3f,"req_per_sec_max":%.3f,"req_per_sec_mean":%.3f,"req_per_sec_stdev":%.3f,' ..
    '"errors":%d,"errors_connect":%d,"errors_read":%d,"errors_write":%d,' ..
    '"errors_status":%d,"errors_timeout":%d}\n',
    summary.requests, seconds, summary.bytes,
    summary.requests / seconds, summary.bytes / seconds,
    latency.min, latency.max, latency.mean, latency.stdev,
    latency:percentile(50.0), latency:percentile(75.0), latency:percentile(90.0),
    latency:percentile(95.0), latency:percentile(99.0),
    requests.min, requests.max, requests.mean, requests.stdev,
    errors, connect, read, write, status, timeout
  ))
end
