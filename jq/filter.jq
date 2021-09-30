import "lecmd" as lecmd;
import "jlecmd" as jlecmd;
import "pecmd" as pecmd;
import "mftecmd" as mftecmd;
import "syslog" as syslog;

def empty_paths: paths(. == null or . == {} or . == [] or . == "");

def del_nulls: until(isempty(. | empty_paths); . | delpaths([. | empty_paths]));

def set_timestamp: .["@timestamp"] |= (. // "0001-01-01T00:00:00.000Z");

if .plugin == "lecmd" then lecmd::transform
elif .plugin == "jlecmd" then jlecmd::transform
elif .plugin == "pecmd" then pecmd::transform
elif .plugin == "mftecmd" then mftecmd::transform
elif .type == "application/syslog" then syslog::transform
else .
end
| del_nulls
| set_timestamp
