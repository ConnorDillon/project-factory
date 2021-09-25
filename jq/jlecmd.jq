import "lecmd" as lecmd;

def timestamp:
  try
    ( sub("\\/Date\\((?<x>-?.{10}).*"; "\(.x)") |
      ( if startswith("-")
        then null
        else tonumber | strftime("%Y-%m-%dT%H:%M:%SZ")
        end
      )
    )
  catch
    null
;

def lnk:
  { target_created: .Header.TargetCreationDate | timestamp
  , target_modified: .Header.TargetModificationDate | timestamp
  , target_accessed: .Header.TargetLastAccessedDate | timestamp
  , file_size: .Header.FileSize | (if (. == 0) then null else . end)
  , local_path: .LocalPath
  , common_path: .CommonPath
  , arguments: .Arguments
  , drive_type: .VolumeInfo.DriveType
  , volume_serial_number: .VolumeInfo.VolumeSerialNumber
  , name: .Name
  # , header_flags: .Header.DataFlags
  # , file_attributes: .Header.FileAttributes | (if (. == 0) then null else . end)
  }
;

def custom_destinations:
  . as $x |
  .data.Entries[] as $e |
  $e.LnkFiles[] |
  { jumplist:
    { app_id: $x.data.AppId.Description
    , rank: $e.Rank
    , name: $e.Name
    }
  , lnk: (. | lnk)
  }
;

def automatic_destinations:
  . as $x |
  .data.DestListEntries[] |
  { jumplist:
    { app_id: $x.data.AppId.Description
    , path: .Path
    , created_on: .CreatedOn | timestamp
    }
  , lnk: (.Lnk | lnk)
  }
;

def transform:
  . as $x |
  ( if .data.Entries
    then custom_destinations
    else automatic_destinations
    end
  ) |
  ., (. | lecmd::lnk_events) |
  .log.file.path |= $x.path
;
