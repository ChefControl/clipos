# A plan for the Infra workflow's log and pull request comment, which are public: tofu's
# "Plan: …" line, then one line per resource that changes, with its address and action
# and none of its attributes. Reads `tofu show -json <planfile>`.
#
# An instance key that isn't a plain word (an IP address, a resource name) shows as [...].
def address:
  gsub("\\[\"(?<key>[^\"]*)\"\\]";
    if (.key | test("^[a-z0-9_]+$")) then "[\"\(.key)\"]" else "[...]" end);

def action:
  .change.actions as $a
  | if $a == ["create"] then "will be created"
    elif $a == ["update"] then "will be updated in-place"
    elif $a == ["delete"] then "will be destroyed"
    elif ($a | index("create")) and ($a | index("delete")) then "must be replaced"
    elif $a == ["forget"] then "will be removed from the state, not destroyed"
    elif .change.importing then "will be imported"
    elif .previous_address then "has moved from \(.previous_address | address)"
    else $a | join(", ")
    end;

[.resource_changes[]?
  | select(.mode == "managed")
  | select(.change.actions != ["no-op"] or .change.importing or .previous_address)] as $changes
| [$changes[].change.actions] as $actions
| [(.output_changes // {}) | to_entries[] | select(.value.actions != ["no-op"]) | .key] as $outputs
| if ($changes | length) == 0 and ($outputs | length) == 0 then
    "No changes."
  else
    "Plan: "
      + ([$changes[] | select(.change.importing)] | length
         | if . > 0 then "\(.) to import, " else "" end)
      + "\([$actions[] | select(index("create"))] | length) to add, "
      + "\([$actions[] | select(. == ["update"])] | length) to change, "
      + "\([$actions[] | select(index("delete"))] | length) to destroy"
      + ([$actions[] | select(index("forget"))] | length
         | if . > 0 then ", \(.) to forget" else "" end)
      + ".",
    ($changes[] | "# \(.address | address) \(action)"),
    if ($outputs | length) > 0 then "Outputs that change: \($outputs | join(", "))" else empty end
  end
