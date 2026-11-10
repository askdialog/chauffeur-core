// The permission mode of a chat, next to the model picker in the composer and in the new
// workspace dialog.

import type { PermissionMode } from "@chauffeur/protocol";
import { PERMISSION_MODES, shownPermissionMode } from "@/lib/permissions";
import { Select, type SelectOption } from "./ui";

const OPTIONS: Array<SelectOption<PermissionMode>> = PERMISSION_MODES.map(({ value, label, name, description, icon: Icon, risky }) => ({
  value,
  label,
  name,
  description,
  icon: <Icon size={13} className="shrink-0" />,
  warning: risky,
}));

export function PermissionPicker({
  value,
  effective,
  model,
  onChange,
  disabled,
}: {
  value: PermissionMode;
  /** The mode the agent reported running in, when it did. */
  effective?: PermissionMode | null;
  model?: string | null;
  onChange: (mode: PermissionMode) => void;
  disabled?: boolean;
}) {
  const shown = shownPermissionMode(value, effective, model);
  const Icon = shown.mode.icon;
  return (
    <Select
      value={value}
      options={OPTIONS}
      onChange={onChange}
      title="Permission mode"
      shown={
        shown.hint
          ? { label: shown.mode.label, icon: <Icon size={13} className="shrink-0" />, hint: shown.hint, tooltip: shown.tooltip ?? undefined }
          : undefined
      }
      testId="permission-picker"
      width={280}
      disabled={disabled}
    />
  );
}
