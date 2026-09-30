import { useState } from 'react'
import type { DngLook } from '@shared/types'
import { ipc } from '../lib/ipc'
import { basename } from '../lib/path'
import { t } from '../lib/strings'
import { Button } from './ui/button'
import { Select } from './ui/select'
import { Row } from './ui/settingsLayout'

export function DngLookPicker({
  value,
  onChange
}: {
  value: DngLook
  onChange: (look: DngLook) => void
}): React.JSX.Element {
  const [error, setError] = useState('')

  async function pickCustom(): Promise<void> {
    setError('')
    try {
      const path = await ipc.invoke('dialog:pickDngLook')
      if (path) onChange({ kind: 'custom', path })
    } catch (error) {
      setError(String(error))
    }
  }

  return (
    <div className="space-y-2">
      <Row label={t('settings.dng_look')} help={t('settings.dng_look.help')}>
        <Select<DngLook['kind']>
          aria-label={t('settings.dng_look')}
          value={value.kind}
          options={[
            { value: 'none', label: t('settings.dng_look.none') },
            { value: 'merrillSpp10', label: 'Merrill SPP 1.0' },
            { value: 'custom', label: t('settings.dng_look.custom') }
          ]}
          onValueChange={(kind) => {
            setError('')
            if (kind === 'custom') void pickCustom()
            else onChange({ kind })
          }}
        />
      </Row>
      {value.kind === 'custom' && (
        <div className="flex min-w-0 items-center justify-between gap-2">
          <span className="truncate font-mono text-xs text-neutral-400" title={value.path}>
            {basename(value.path)}
          </span>
          <Button variant="bordered" size="sm" onClick={() => void pickCustom()}>
            {t('button.browse')}
          </Button>
        </div>
      )}
      {value.kind === 'merrillSpp10' && (
        <p className="text-xs text-neutral-500">{t('settings.dng_look.merrill_help')}</p>
      )}
      {error && (
        <p role="alert" className="text-xs text-red-400">
          {error}
        </p>
      )}
    </div>
  )
}
