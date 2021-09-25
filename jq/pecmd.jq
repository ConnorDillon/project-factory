def timestamp:
  try sub("(?<x>.*\\.\\d{3})\\d{4}(?<y>.*)"; "\(.x)\(.y)") catch null
;

def transform:
  { prefetch:
    { runs:
      [ .data.LastRun
      , . as $x | range(7) | $x.data["PreviousRun\(.)"] | select(.)
      ] |
      map(timestamp)
    , volumes:
      [ . as $x |
        range(5) |
        { created: $x.data["Volume\(.)Created"] | timestamp
        , name: $x.data["Volume\(.)Name"]
        , serial: $x.data["Volume\(.)Serial"]
        } |
        select(.created)
      ]
    , run_count: .data.RunCount | tonumber
    , executable_name: .data.ExecutableName
    , hash: .data.Hash
    , version: .data.Version
    , directories: .data.Directories | split(", ")
    , files_loaded: .data.FilesLoaded | split(", ")
    }
  , file:
    { path: .path
    , mime_type: .type
    }
  } as $pf |

  .
  , ( $pf.prefetch.runs[] |
      { "@timestamp": .
      , event:
        { kind: "event"
        , category: "process"
        , type: "start"
        , action: "process-start"
        , outcome: "success"
        }
      , log: {file: {path: $pf.file.path}}
      , file: {name: $pf.prefetch.executable_name}
      , process:
        { name: $pf.prefetch.executable_name
        , start: .
        }
      }
    )
;
