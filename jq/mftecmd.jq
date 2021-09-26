def timestamp:
  try sub("(?<x>.*\\.\\d{3})\\d{4}(?<y>.*)"; "\(.x)\(.y)") catch null
;

def file_events:
  . as $x |
  { file:
    { type: .file.type
    , path: .file.path
    , name: .file.name
    , extension: .file.extension
    , directory: .file.directory
    , size: .file.size
    }
  , event:
    { kind: "event"
    , category: "file"
    , outcome: "success"
    }
  } |
  . * {"@timestamp": $x.file.mtime, event: {type: "change", action: "file-modified"}}
  , . * {"@timestamp": $x.file.created, event: {type: "creation", action: "file-created"}}
  , . * {"@timestamp": $x.file.ctime, event: {type: "change", action: "file-meta-changed"}}
  , . * {"@timestamp": $x.file.accessed, event: {type: "access", action: "file-accessed"}}
;

def transform:
  . as $x |
  .data |
  { file:
    { accessed: .LastAccess0x10 | timestamp
    , created:.Created0x10 | timestamp
    , ctime: .LastRecordChange0x10 | timestamp
    , directory: .ParentPath
    , extension: .Extension
    , inode: .EntryNumber
    , mtime: .LastModified0x10 | timestamp
    , path: "\(.ParentPath)\\\(.FileName)"
    , name: .FileName
    , size: .FileSize
    , type: (if .IsDirectory then "dir" else "file" end)
    }
  , mft:
    { log_file_sequence_number: .LogfileSequenceNumber
    , is_ads: .IsAds
    , in_use: .InUse
    , parent_entry_number: .ParentEntryNumber
    , copied: .Copied
    , fn_attribute_id: .FnAttributeId
    , has_ads: .HasAds
    , name_type: .NameType
    , other_attribute_id: .OtherAttributeId
    , parent_sequence_number: .ParentSequenceNumber
    , reference_count: .ReferenceCount
    , security_id: .SecurityId
    , sequence_number: .SequenceNumber
    , si_flags: .SiFlags
    , timestomped: .Timestomped
    , update_sequence_number: .UpdateSequenceNumber
    , usec_zeros: .uSecZeros
    }
  } |
  ., (. | file_events) |
  .log.file.path |= $x.path
;
