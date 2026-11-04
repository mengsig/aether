import { useId } from "react";
import { invariant } from "./invariant.ts";
import { FILTER_KEYS, type Facets, type FilterKey, type FleetFilter } from "./rows.ts";

const LABELS: Readonly<Record<FilterKey, string>> = { run: "Run", role: "Role", state: "State", model: "Model" };

function FilterSelect(props: { readonly name: FilterKey; readonly facets: Facets; readonly value: string; readonly onPick: (value: string) => void }) {
  const id = useId();
  const options = props.facets[props.name];
  // A chosen value that has since left the fleet stays listed, so the select never shows a value it lacks.
  const missing = props.value !== "" && !options.some((option) => option.value === props.value);
  return (
    <div className="ui-field fleet-filter">
      <label className="ui-field-label" htmlFor={id}>{LABELS[props.name]}</label>
      <select id={id} className="ui-field-input" value={props.value} onChange={(event) => props.onPick(event.currentTarget.value)}>
        <option value="">All</option>
        {missing ? <option value={props.value}>{props.value} (0)</option> : null}
        {options.map((option) => <option key={option.value} value={option.value}>{option.label} ({option.count})</option>)}
      </select>
    </div>
  );
}

/** One select per filter; each lists the values the fleet holds now. */
export function FleetFilters(props: { readonly facets: Facets; readonly filter: FleetFilter; readonly onChange: (next: FleetFilter) => void }) {
  invariant(FILTER_KEYS.every((key) => typeof props.filter[key] === "string"), "every filter holds a string");
  return (
    <div className="fleet-filters" role="group" aria-label="Filter agents">
      {FILTER_KEYS.map((key) => (
        <FilterSelect key={key} name={key} facets={props.facets} value={props.filter[key]}
          onPick={(value) => props.onChange({ ...props.filter, [key]: value })} />
      ))}
    </div>
  );
}
