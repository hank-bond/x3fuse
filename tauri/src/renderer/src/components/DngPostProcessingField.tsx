import { useId } from 'react'
import { t } from '../lib/strings'

export function DngPostProcessingField({
  value,
  onChange
}: {
  value: string
  onChange: (command: string) => void
}): React.JSX.Element {
  const id = useId()
  return (
    <div className="space-y-2">
      <label htmlFor={id} className="text-sm text-neutral-300">
        {t('settings.dng_post_processing.command')}
      </label>
      <textarea
        id={id}
        aria-describedby={`${id}-help`}
        className="min-h-20 w-full rounded border border-neutral-700 bg-neutral-900 p-2 font-mono text-xs text-neutral-100 focus:border-neutral-500 focus:outline-none"
        rows={3}
        spellCheck={false}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
      <p id={`${id}-help`} className="text-xs text-neutral-500">
        {t('settings.dng_post_processing.help')}
      </p>
    </div>
  )
}
