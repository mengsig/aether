import { Picker, type PickerOption } from "@aether-ui/ui";
import { useMemo } from "react";
import { invariant } from "./invariant.ts";

export interface SelectOption { readonly value: string; readonly label: string }

/** A labelled single choice, drawn by the kit's Picker; `onChange` still receives the option's value. */
export function SelectField({ label, value, options, onChange, className }: {
  readonly label: string;
  readonly value: string;
  readonly options: readonly SelectOption[];
  readonly onChange: (value: string) => void;
  readonly className?: string;
}) {
  invariant(options.some((option) => option.value === value), `${label} holds one of its options`);
  invariant(new Set(options.map((option) => option.value)).size === options.length, `${label} offers each value once`);
  const choices = useMemo<PickerOption[]>(() => options.map((option) => ({ id: option.value, label: option.label })), [options]);
  return <Picker className={className} label={label} value={value} options={choices} onChange={onChange} />;
}
