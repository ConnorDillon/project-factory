def timestamp:
"(\\w{3}\\s)?
(?<timestamp>\\w{3}\\s+\\d+\\s\\d{2}:\\d{2}:\\d{2})
(\\.\\d{3})? \\s+
";

def kernel_regex:
"\(timestamp)
<(?<process>kernel)>
\\s (?<message>.*)
";

def regex:
"(\(timestamp))
((?<host>\\S+) \\s)?
<? (?<process>[^\\[]+)
\\[ (?<pid>\\d+) \\] >?
.*:\\s (?<message>.*)
";

def parse:
  (. | capture(kernel_regex; "x"))
  // (. | capture(regex; "x"))
  // (. | capture(timestamp + "(?<message>.*)"; "x"))
  // {message: .}
; 

def fmt_timestamp:
  . as $x
  | (now | gmtime) as $now
  | $x
  | strptime("%b %d %H:%M:%S")
  | ( if .[1] > $now[1]
      then (.[0] |= $now[0] - 1)
      else (.[0] |= $now[0])
      end
    )
  | strftime("%Y-%m-%dT%H:%M:%S")
;

def transform:
  . as $x
  | .data 
  | parse 
  | { "@timestamp": (try (.timestamp | fmt_timestamp) catch null)
    , "@host": .host
    , message: .message
    , process:
      { name: .process
      , pid: .pid
      }
    , log: {file: {path: $x.path}}
    , event:
      { original: $x.data
      }
    }
;
