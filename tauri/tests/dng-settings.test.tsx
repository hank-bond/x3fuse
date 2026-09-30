// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { batchSettings, DEFAULT_SETTINGS, type ConversionSettings } from '@shared/types'
import { SettingsWindow } from '../src/renderer/src/components/SettingsWindow'
import { DngLookPicker } from '../src/renderer/src/components/DngLookPicker'
import { useSettingsStore } from '../src/renderer/src/stores/settingsStore'

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock('../src/renderer/src/lib/ipc', () => ({ ipc: { invoke } }))

let saved: ConversionSettings
let choice: string | null
let selectedFile: string | null

beforeEach(() => {
  saved = structuredClone(DEFAULT_SETTINGS)
  choice = null
  selectedFile = null
  useSettingsStore.setState({ settings: saved, loaded: false })
  invoke.mockImplementation(async (channel, patch) => {
    if (channel === 'settings:get') return saved
    if (channel === 'settings:set') {
      saved = { ...saved, ...patch }
      return saved
    }
    if (channel === 'app:info') return { version: '0.1.0' }
    if (channel === 'logs:sizes') return { conversion: 0, error: 0, debug: 0 }
    if (channel === 'menu:popup') return choice
    if (channel === 'dialog:pickDngLook') return selectedFile
  })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

async function select(kind: string): Promise<void> {
  choice = kind
  const buttons = await screen.findAllByRole('button', { name: 'DNG look', exact: true })
  const menu = buttons.find((button) => button.getAttribute('aria-haspopup') === 'menu')!
  fireEvent.click(menu)
}

it('starts off and saves bundled, custom and disabled selections from Settings without exporting', async () => {
  let view = render(<SettingsWindow />)
  await screen.findByText('None')
  expect(saved.dngLook).toEqual({ kind: 'none' })
  await select('merrillSpp10')
  await waitFor(() => expect(saved.dngLook).toEqual({ kind: 'merrillSpp10' }))
  expect(invoke).toHaveBeenCalledWith('settings:set', { dngLook: { kind: 'merrillSpp10' } })

  view.unmount()
  useSettingsStore.setState({ settings: structuredClone(DEFAULT_SETTINGS), loaded: false })
  view = render(<SettingsWindow />)
  await screen.findByText('Merrill SPP 1.0')

  selectedFile = '/profiles/色 look.dcp'
  await select('custom')
  await waitFor(() => expect(saved.dngLook).toEqual({ kind: 'custom', path: selectedFile }))
  expect(screen.getByTitle(selectedFile)).toBeTruthy()
  const captured = batchSettings(saved)
  await select('none')
  await waitFor(() => expect(saved.dngLook).toEqual({ kind: 'none' }))
  expect(captured.dngLook).toEqual({ kind: 'custom', path: selectedFile })
  expect(saved.hasPreviousConversion).toBe(false)
  view.unmount()
})

it('persists a command independently of the look and freezes it for a batch', async () => {
  const view = render(<SettingsWindow />)
  const field = await screen.findByRole('textbox', { name: 'Command' })
  expect((field as HTMLTextAreaElement).value).toBe('')
  const command = '"/tools/色 python" "/scripts/preview.py"'
  fireEvent.change(field, { target: { value: command } })
  await waitFor(() => expect(saved.dngPostProcessingCommand).toBe(command))
  expect(saved.dngLook).toEqual({ kind: 'none' })
  expect(saved.hasPreviousConversion).toBe(false)
  const captured = batchSettings(saved)
  view.unmount()
  useSettingsStore.setState({ settings: structuredClone(DEFAULT_SETTINGS), loaded: false })
  render(<SettingsWindow />)
  const restored = await screen.findByRole('textbox', { name: 'Command' })
  expect((restored as HTMLTextAreaElement).value).toBe(command)
  fireEvent.change(restored, { target: { value: '' } })
  await waitFor(() => expect(saved.dngPostProcessingCommand).toBe(''))
  expect(captured.dngPostProcessingCommand).toBe(command)
})

it('keeps the current look when the file dialog is cancelled and reports dialog errors', async () => {
  const onChange = vi.fn()
  render(<DngLookPicker value={{ kind: 'merrillSpp10' }} onChange={onChange} />)
  await select('custom')
  await waitFor(() => expect(invoke).toHaveBeenCalledWith('dialog:pickDngLook'))
  expect(onChange).not.toHaveBeenCalled()
  invoke.mockImplementation(async (channel) => {
    if (channel === 'menu:popup') return 'custom'
    if (channel === 'dialog:pickDngLook') throw new Error('Dialog unavailable')
  })
  await select('custom')
  expect((await screen.findByRole('alert')).textContent).toContain('Dialog unavailable')
  expect(onChange).not.toHaveBeenCalled()
})
