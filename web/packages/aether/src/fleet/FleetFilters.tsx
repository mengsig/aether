import { Picker } from "@aether-ui/ui";
import { invariant } from "./invariant.ts";
import { FILTER_KEYS, type Facets, type FilterKey, type FleetFilter } from "./rows.ts";

const LABELS: Readonly<Record<FilterKey, string>> = { run: "Run", role: "Role", state: "State", model: "Model" };
// A select's values are strings: "" is All, and every real value (the empty role too) is prefixed.
const ALL = "";
const encode = (value: string | null): string => (value === null ? ALL : `=${value}`);
const decode = (option: string): string | null => (option === ALL ? null : option.slice(1));

function FilterSelect(props: { readonly name: FilterKey; readonly facets: Facets; readonly value: string | null; readonly onPick: (value: string | null) => void }) {
  const options = props.facets[props.name];
  // A chosen value that has since left the fleet stays listed, so the select never shows a value it lacks.
  const missing = props.value !== null && !options.some((option) => option.value === props.value);
  return <Picker label={LABELS[props.name]} size="sm" value={encode(props.value)} onChange={value => props.onPick(decode(value))}
    options={[{ id: ALL, label: "All" }, ...(missing ? [{ id: encode(props.value), label: `${props.value} (0)` }] : []),
      ...options.map(option => ({ id: encode(option.value), label: `${option.label} (${option.count})` }))]} />;
}

/** One select per filter; each lists the values the fleet holds now. */
export function FleetFilters(props: { readonly facets: Facets; readonly filter: FleetFilter; readonly onChange: (next: FleetFilter) => void }) {
  invariant(FILTER_KEYS.every((key) => props.filter[key] === null || typeof props.filter[key] === "string"), "every filter holds a value or null");
  return (
    <div className="fleet-filters" role="group" aria-label="Filter agents">
      {FILTER_KEYS.map((key) => (
        <FilterSelect key={key} name={key} facets={props.facets} value={props.filter[key]}
          onPick={(value) => props.onChange({ ...props.filter, [key]: value })} />
      ))}
    </div>
  );
}
