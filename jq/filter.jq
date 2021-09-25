import "lecmd" as lecmd;
import "jlecmd" as jlecmd;
import "pecmd" as pecmd ;

def empty_paths: paths(try ((. | length) == 0) catch false);

def del_nulls: until(isempty(. | empty_paths); . | delpaths([. | empty_paths]));

if .plugin == "lecmd" then lecmd::transform
elif .plugin == "jlecmd" then jlecmd::transform
elif .plugin == "pecmd" then pecmd::transform
else .
end |
del_nulls
