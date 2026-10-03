import type { ReactNode } from "react";

// An on/off setting with a label and an optional line under it ("Recorded from my point
// of view"). The whole row is the tap target.
export function Switch({
  checked,
  onChange,
  label,
  hint,
  tone = "accent",
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: ReactNode;
  hint?: ReactNode;
  tone?: "accent" | "violet";
  disabled?: boolean;
}) {
  const on = tone === "violet" ? "bg-violet" : "bg-accent";
  return (
    <label
      className={`flex cursor-pointer items-start gap-3 py-1 ${disabled ? "cursor-default opacity-50" : ""}`}
    >
      <input
        type="checkbox"
        role="switch"
        aria-checked={checked}
        className="peer sr-only"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span
        aria-hidden="true"
        className={`relative mt-0.5 block h-6 w-10 shrink-0 rounded-full transition peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent ${
          checked ? on : "bg-white/15"
        }`}
      >
        <span
          className={`absolute top-0.5 left-0.5 block h-5 w-5 rounded-full bg-white shadow transition-transform ${
            checked ? "translate-x-4" : ""
          }`}
        />
      </span>
      <span className="flex flex-col gap-0.5">
        <span className="font-semibold">{label}</span>
        {hint && <span className="text-sm text-muted">{hint}</span>}
      </span>
    </label>
  );
}
