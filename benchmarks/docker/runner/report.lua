done = function(summary, latency, requests)
  local seconds = summary.duration / 1000000
  local errors = summary.errors.connect + summary.errors.read +
                 summary.errors.write + summary.errors.status + summary.errors.timeout
  io.write(string.format(
    'WRK_JSON:{"requests":%d,"duration_seconds":%.6f,"requests_per_second":%.3f,' ..
    '"bytes_per_second":%.3f,"latency_us_p50":%.3f,"latency_us_p95":%.3f,' ..
    '"latency_us_p99":%.3f,"errors":%d}\n',
    summary.requests, seconds, summary.requests / seconds, summary.bytes / seconds,
    latency:percentile(50.0), latency:percentile(95.0), latency:percentile(99.0), errors
  ))
end

