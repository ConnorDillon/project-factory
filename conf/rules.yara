rule GZip {
    meta: type = "application/gzip"
    strings: $sig = { 1F 8B }
    condition: $sig at 0
}

rule Tarball {
    meta: type = "application/x-tar"
    strings: $sig = "ustar"
    condition: $sig at 257
}

rule MFT {
    meta: type = "ntfs/mft"
    strings: $sig = "FILE"
    condition: $sig at 0 
}

rule Prefetch {
    meta: type = "windows/prefetch"
    strings: $sig = { 4d 41 4d 04 }
    condition: $sig at 0 
}

rule LnkFile {
    meta: type = "windows/lnk"
    strings: $lnk_header = { 4C 00 00 00 01 14 02 00 00 00 00 00 C0 00 00 00 00 00 00 46 }
    condition: $lnk_header at 0
}

rule JumpListAutomaticDestinations {
    meta: type = "windows/jumplist.automatic_destinations"
    strings:
        $olecf_sig = { d0 cf 11 e0 a1 b1 1a e1 }
        $lnk_header = { 4C 00 00 00 01 14 02 00 00 00 00 00 C0 00 00 00 00 00 00 46 }
    condition: $olecf_sig at 0 and $lnk_header in ( 8..4096 )
}

rule JumpListCustomDestinations {
    meta: type = "windows/jumplist.custom_destinations"
    strings:
        $cd_header = { 02 00 00 00 ( 01 | 02 ) 00 00 00 }
        $lnk_header = { 4C 00 00 00 01 14 02 00 00 00 00 00 C0 00 00 00 00 00 00 46 }
    condition: $cd_header at 0 and $lnk_header in ( 8..4096 )
}
