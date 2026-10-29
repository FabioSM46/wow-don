import { useId, type ReactNode } from "react";

/**
 * The shared parts of the Options tab (issue #231): a section with its title, a panel of
 * rows and a row with label and description on the left and the control on the right. Only
 * presentation: state and saves stay in the section components.
 */

type SectionProps = {
  /** The id of the title, which names the section (`aria-labelledby`). */
  id: string;
  title: string;
  /** The main button of the section, next to the title. */
  action?: ReactNode;
  className?: string;
  busy?: boolean;
  children: ReactNode;
};

export function SettingsSection({ id, title, action, className, busy, children }: SectionProps) {
  return (
    <section
      className={className ? `settings-section ${className}` : "settings-section"}
      aria-labelledby={id}
      aria-busy={busy}
    >
      <header className="settings-section-header">
        <h3 id={id}>{title}</h3>
        {action}
      </header>
      {children}
    </section>
  );
}

type RowProps = {
  label: ReactNode;
  /** The id of the control, when the label is a `<label>` that names it. */
  htmlFor?: string;
  /** The id of the label, for a control named with `aria-labelledby` (a group). */
  labelId?: string;
  description?: ReactNode;
  descriptionId?: string;
  children?: ReactNode;
};

/** A row of the panel: label and description on the left, control on the right. */
export function SettingRow({ label, htmlFor, labelId, description, descriptionId, children }: RowProps) {
  return (
    <div className="setting">
      <div className="setting-text">
        {htmlFor ? (
          <label className="setting-label" htmlFor={htmlFor} id={labelId}>
            {label}
          </label>
        ) : (
          <span className="setting-label" id={labelId}>
            {label}
          </span>
        )}
        {description && (
          <span className="setting-description" id={descriptionId}>
            {description}
          </span>
        )}
      </div>
      {children && <div className="setting-control">{children}</div>}
    </div>
  );
}

type SwitchProps = {
  label: ReactNode;
  description?: ReactNode;
  /** A description shared by several rows, already on the page, in place of `description`. */
  describedBy?: string;
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
};

/** A yes/no row: the `.switch` of D42, named by its label. */
export function SwitchRow({ label, description, describedBy, checked, disabled, onChange }: SwitchProps) {
  const id = useId();
  const descriptionId = description ? `${id}-description` : describedBy;
  return (
    <SettingRow
      label={label}
      htmlFor={id}
      description={description}
      descriptionId={description ? descriptionId : undefined}
    >
      <input
        id={id}
        type="checkbox"
        role="switch"
        className="switch"
        checked={checked}
        aria-checked={checked}
        disabled={disabled}
        aria-describedby={descriptionId}
        onChange={(event) => onChange(event.target.checked)}
      />
    </SettingRow>
  );
}

type SegmentedProps<T extends string> = {
  /** The id of the label that names the group. */
  labelledBy: string;
  describedBy?: string;
  options: readonly { id: T; label: string }[];
  value: T | null;
  disabled: boolean;
  onChange: (value: T) => void;
};

/**
 * A choice among a few items: the `.segmented` of D42, a group of buttons with the chosen one
 * in `aria-pressed`. Each button is reached with Tab and chosen with Enter or Space.
 */
export function Segmented<T extends string>({
  labelledBy,
  describedBy,
  options,
  value,
  disabled,
  onChange,
}: SegmentedProps<T>) {
  return (
    <div className="segmented" role="group" aria-labelledby={labelledBy} aria-describedby={describedBy}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          aria-pressed={option.id === value}
          disabled={disabled}
          onClick={() => {
            if (option.id !== value) onChange(option.id);
          }}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
