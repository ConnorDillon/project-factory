def timestamp:
  try sub("(?<x>.*\\.\\d{3})\\d{4}(?<y>.*)"; "\(.x)\(.y)") catch null
;

def lnk_events:
  . as $lnk |
  { file:
    { type:
      ( try (if .lnk.file_attributes | test("FileAttributeDirectory") then "dir" else "file" end)
        catch null
      )
    , path: .lnk.local_path
    , size: .lnk.file_size
    }
  , log: {file: {path: .file.path}}
  , event:
    { kind: "event"
    , category: "file"
    , outcome: "success"
    }
  } |
  ( . * {"@timestamp": $lnk.lnk.target_created, event: {type: "creation", action: "file-created"}}
  , . * {"@timestamp": $lnk.lnk.target_modified, event: {type: "change", action: "file-modified"}}
  , . * {"@timestamp": $lnk.lnk.target_accessed, event: {type: "access", action: "file-accessed"}}
  ) |
  select(.["@timestamp"])
;

def transform:
  { lnk:
    { target_created: .data.TargetCreated | timestamp
    , target_modified: .data.TargetModified | timestamp
    , target_accessed: .data.TargetAccessed | timestamp
    , drive_type: .data.DriveType
    , extrab_locks_present: .data.ExtraBlocksPresent
    , file_attributes: .data.FileAttributes
    , file_size: .data.FileSize
    , header_flags: .data.HeaderFlags
    , mac_vendor: .data.MACVendor
    , machine_id: .data.MachineID
    , machine_mac_address: .data.MachineMACAddress
    , relative_path: .data.RelativePath
    , local_path: .data.LocalPath
    , target_id_absolute_path: .data.TargetIDAbsolutePath
    , target_mft_entry_number: .data.TargetMFTEntryNumber
    , target_mft_sequence_number: .data.TargetMFTSequenceNumber
    , volume_serial_number: .data.VolumeSerialNumber
    , local_path: .data.LocalPath
    }
  , file:
    { path: .path
    , target_path: .data.LocalPath
    , type: "symlink"
    }
  } |
  ., (. | lnk_events)
;
