import { useId, type ReactNode } from "react";

/**
 * Le parti comuni della tab Options (issue #231): una sezione con il suo titolo, una scheda di
 * righe e una riga con etichetta e descrizione a sinistra e il controllo a destra. Solo
 * presentazione: lo stato e i salvataggi restano nei componenti delle sezioni.
 */

type SectionProps = {
  /** L'id del titolo, che dà il nome alla sezione (`aria-labelledby`). */
  id: string;
  title: string;
  /** Il pulsante principale della sezione, accanto al titolo. */
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
  /** L'id del controllo, quando l'etichetta è un `<label>` che lo nomina. */
  htmlFor?: string;
  /** L'id dell'etichetta, per un controllo che si nomina con `aria-labelledby` (un gruppo). */
  labelId?: string;
  description?: ReactNode;
  descriptionId?: string;
  children?: ReactNode;
};

/** Una riga della scheda: etichetta e descrizione a sinistra, controllo a destra. */
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
  /** Una descrizione comune a più righe, già nella pagina, al posto di `description`. */
  describedBy?: string;
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
};

/** Una riga sì/no: l'interruttore `.switch` di D42, nominato dalla sua etichetta. */
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
  /** L'id dell'etichetta che dà il nome al gruppo. */
  labelledBy: string;
  describedBy?: string;
  options: readonly { id: T; label: string }[];
  value: T | null;
  disabled: boolean;
  onChange: (value: T) => void;
};

/**
 * Una scelta tra poche voci: il `.segmented` di D42, un gruppo di pulsanti con quello scelto in
 * `aria-pressed`. Ogni pulsante si raggiunge con Tab e si sceglie con Invio o Spazio.
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
