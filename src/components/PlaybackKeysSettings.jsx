import { useEffect, useId, useState } from 'react'
import { AlertTriangle } from 'lucide-react'
import { Card, CardHeader, CardTitle, CardContent } from './ui/card'
import { Button } from './ui/button'
import { Switch } from './ui/switch'
import { Segmented } from './ui/segmented'
import {
  PLAYBACK_KEY_ACTIONS,
  SUGGESTED_PLAYBACK_KEYS,
  acceleratorFromKeyEvent,
  actionUsingKey,
  backAmount,
  formatPlaybackKey,
  playbackKeyNote,
} from '../lib/playbackKeys'
import { backstepLabel, loadBackstep } from '../lib/player'

function readBackstep() {
  try {
    return loadBackstep(window.localStorage.getItem('player-backstep'))
  } catch {
    return loadBackstep(null)
  }
}

function KeyCap({ children }) {
  return (
    <kbd className="inline-flex items-center rounded border border-border bg-secondary/60 px-1.5 py-px font-sans type-small font-medium tabular-nums text-foreground">
      {children}
    </kbd>
  )
}

function rowStatus({ accelerator, enabled, suspended, registration }) {
  if (!accelerator) return { text: 'Not set', tone: 'sub' }
  if (!enabled) return { text: 'Not registered (off)', tone: 'sub' }
  if (suspended) return { text: 'Paused while you assign a key', tone: 'sub' }
  if (!registration) return { text: 'Registering…', tone: 'sub' }
  if (registration.state === 'registered') return { text: 'Registered', tone: 'ok' }
  return { text: `Couldn’t register: ${registration.error}`, tone: 'error' }
}

function KeyRow({ action, accelerator, enabled, keys, onRecord, onClear, recording, onRecorded, onCancel }) {
  const [error, setError] = useState('')
  const status = rowStatus({
    accelerator,
    enabled,
    suspended: keys.suspended,
    registration: keys.registration[action.id],
  })
  const note = accelerator ? playbackKeyNote(accelerator) : ''
  const suggested = SUGGESTED_PLAYBACK_KEYS[action.id]
  const statusId = useId()

  const onKeyDown = event => {
    if (!recording) return
    if (event.key === 'Tab') return
    event.preventDefault()
    event.stopPropagation()
    const result = acceleratorFromKeyEvent(event)
    if (result.pending) return
    if (result.cancel) {
      setError('')
      onCancel()
      return
    }
    if (result.error) {
      setError(result.error)
      return
    }
    const owner = actionUsingKey(keys.settings.bindings, result.accelerator, action.id)
    if (owner) {
      const ownerLabel = PLAYBACK_KEY_ACTIONS.find(a => a.id === owner)?.label ?? owner
      setError(`${formatPlaybackKey(result.accelerator)} is already assigned to ${ownerLabel}.`)
      return
    }
    setError('')
    onRecorded(result.accelerator)
  }

  return (
    <li className="grid gap-1.5 border-t border-border py-2.5 first:border-t-0 sm:grid-cols-[150px_minmax(0,1fr)_auto] sm:items-center">
      <div className="flex flex-col">
        <span className="text-[12px] font-medium text-foreground">{action.label}</span>
        <span className="type-small text-[hsl(var(--sub))]">{action.hint}</span>
      </div>
      <div className="flex min-w-0 flex-col gap-0.5" id={statusId}>
        <div className="flex flex-wrap items-center gap-2">
          {accelerator ? (
            <KeyCap>{formatPlaybackKey(accelerator)}</KeyCap>
          ) : (
            suggested && (
              <span className="type-small text-[hsl(var(--sub))]">Suggested: {formatPlaybackKey(suggested)}</span>
            )
          )}
          <span
            className={
              status.tone === 'error'
                ? 'type-small text-destructive'
                : status.tone === 'ok'
                  ? 'type-small text-foreground'
                  : 'type-small text-[hsl(var(--sub))]'
            }
          >
            {status.text}
          </span>
        </div>
        {note && <span className="type-small text-[hsl(var(--text2))]">{note}</span>}
        {error && (
          <span role="alert" className="type-small text-destructive">
            {error}
          </span>
        )}
      </div>
      <div className="flex items-center gap-1.5">
        <Button
          size="sm"
          variant={recording ? 'default' : 'outline'}
          className={recording ? 'ring-1 ring-inset ring-foreground text-foreground' : undefined}
          aria-describedby={statusId}
          aria-label={
            recording ? `Press a key for ${action.label}. Escape cancels.` : `Assign a key to ${action.label}`
          }
          onClick={() => {
            setError('')
            if (recording) onCancel()
            else onRecord()
          }}
          onKeyDown={onKeyDown}
          onBlur={() => recording && onCancel()}
        >
          {recording ? 'Press a key…' : accelerator ? 'Change' : 'Assign'}
        </Button>
        {accelerator && !recording && (
          <Button size="sm" variant="ghost" aria-label={`Clear the key for ${action.label}`} onClick={onClear}>
            Clear
          </Button>
        )}
      </div>
    </li>
  )
}

/**
 * Settings → Playback keys. `prefs` holds playbackKeys/setPlaybackKeys;
 * `keys` is the usePlaybackKeys() result from the app shell.
 */
export default function PlaybackKeysSettings({ prefs, keys }) {
  const { playbackKeys, setPlaybackKeys } = prefs
  const { enabled, holdToPlay, bindings } = keys.settings
  const [recordingAction, setRecordingAction] = useState(null)
  const [backstep] = useState(readBackstep)
  const switchId = useId()
  const switchHintId = useId()
  const { setSuspended } = keys

  // Release every key while the recorder listens, so the press reaches it
  useEffect(() => {
    setSuspended(recordingAction != null)
    return () => setSuspended(false)
  }, [recordingAction, setSuspended])

  const update = patch => setPlaybackKeys({ ...playbackKeys, ...patch })
  const setBinding = (action, accelerator) => {
    const next = { ...bindings }
    if (accelerator) next[action] = accelerator
    else delete next[action]
    update({ bindings: next })
  }
  const unsetSuggestions = Object.entries(SUGGESTED_PLAYBACK_KEYS).filter(
    ([action, accelerator]) => !bindings[action] && !actionUsingKey(bindings, accelerator, action),
  )
  const assignedCount = Object.keys(bindings).length
  const failed = Object.values(keys.registration).filter(r => r.state === 'failed').length

  let summary
  if (!enabled) summary = 'Off. No keys are taken from other apps.'
  else if (assignedCount === 0) summary = 'On, but no keys are assigned yet.'
  else if (keys.suspended) summary = 'Keys are released while you assign a key.'
  else if (failed > 0) summary = `On. ${failed} of ${assignedCount} keys could not be registered; see below.`
  else summary = `On. ${assignedCount} key${assignedCount === 1 ? '' : 's'} work in every app while DepoAudio is open.`

  return (
    <>
      <Card>
        <CardHeader>
          <CardTitle>Playback keys while typing elsewhere</CardTitle>
        </CardHeader>
        <CardContent className="p-4 flex flex-col gap-3">
          <div className="flex items-start justify-between gap-4">
            <div className="flex min-w-0 flex-col">
              <label htmlFor={switchId} className="text-[12px] font-medium text-foreground">
                Control the Player from other apps
              </label>
              <span id={switchHintId} className="type-small text-[hsl(var(--sub))] mt-0.5">
                Off by default. When on, the keys you assign below play, pause and move through the recording in the
                Player while your CAT software, word processor or any other app is in front.
              </span>
            </div>
            <Switch
              id={switchId}
              aria-describedby={switchHintId}
              checked={enabled}
              onCheckedChange={value => update({ enabled: value })}
            />
          </div>

          <div className="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 type-small text-foreground">
            <AlertTriangle size={14} aria-hidden="true" className="mt-0.5 shrink-0" />
            <span>
              While this is on, DepoAudio takes these keys from every app. Pressing one in your CAT software or word
              processor controls the Player instead and does not reach that app. Turn this off to give the keys back.
            </span>
          </div>

          <p role="status" aria-live="polite" className="type-small text-foreground">
            {summary}
          </p>
          {keys.syncError && (
            <p role="alert" className="type-small text-destructive">
              {keys.syncError}
            </p>
          )}
          {enabled && keys.status && <p className="type-small text-[hsl(var(--text2))]">{keys.status}</p>}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Keys</CardTitle>
          {unsetSuggestions.length > 0 && (
            <Button
              size="sm"
              variant="outline"
              onClick={() => update({ bindings: { ...bindings, ...Object.fromEntries(unsetSuggestions) } })}
            >
              Use suggested keys
            </Button>
          )}
        </CardHeader>
        <CardContent className="p-4 flex flex-col gap-2">
          <p className="type-small text-[hsl(var(--sub))]">
            Choose a function key (F1–F24), or a key together with Ctrl or Cmd. Letters, numbers, Space and Escape on
            their own are refused because they type or cancel in every app. The suggested F13–F15 avoid the keys Express
            Scribe and Case CATalyst use by default; they are on full-size keyboards, and pedals in keyboard mode can
            send them.
          </p>
          <ul className="flex flex-col">
            {PLAYBACK_KEY_ACTIONS.map(action => (
              <KeyRow
                key={action.id}
                action={action}
                accelerator={bindings[action.id]}
                enabled={enabled}
                keys={keys}
                recording={recordingAction === action.id}
                onRecord={() => setRecordingAction(action.id)}
                onCancel={() => setRecordingAction(null)}
                onRecorded={accelerator => {
                  setBinding(action.id, accelerator)
                  setRecordingAction(null)
                }}
                onClear={() => setBinding(action.id, null)}
              />
            ))}
          </ul>
          <p className="type-small text-[hsl(var(--sub))]">
            Back moves {backAmount(backstep)} s (
            {backstep > 0 ? `the Player’s Backstep, ${backstepLabel(backstep)}` : 'Backstep is Off'}). If a key does
            nothing, another app may already have it, and not every system reports that. Choose another key.
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Play key behavior</CardTitle>
        </CardHeader>
        <CardContent className="p-4 flex flex-col gap-2">
          <Segmented
            aria-label="Play key behavior"
            value={holdToPlay ? 'hold' : 'toggle'}
            onChange={value => update({ holdToPlay: value === 'hold' })}
            options={[
              { value: 'toggle', label: 'Press to play or pause' },
              { value: 'hold', label: 'Hold to play' },
            ]}
          />
          <p className="type-small text-[hsl(var(--sub))]">
            {holdToPlay
              ? `Hold to play: the Player plays while the Play/Pause key or pedal is held down and pauses when you let go. When you press again, it starts ${
                  backstep > 0
                    ? `${backstep} s earlier (the Player’s Backstep)`
                    : 'where it paused (the Player’s Backstep is Off)'
                }.`
              : 'Each press plays or pauses, like the Play button.'}
          </p>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Foot pedals</CardTitle>
        </CardHeader>
        <CardContent className="p-4 flex flex-col gap-2 type-small text-[hsl(var(--text2))]">
          <p>
            A pedal in keyboard mode acts like a keyboard and sends the keys you set in its own setup tool, such as the
            OM System (Olympus) Foot Switch Configuration Tool. Set each pedal to send the key assigned here, and turn
            on Hold to play for the classic press-and-hold pedal.
          </p>
          <p>
            Infinity IN-USB pedals are not keyboards. They need a helper app, such as NCH Pedable, to turn each pedal
            into a key. DepoAudio does not read pedals directly yet.
          </p>
          <p>
            On macOS, DepoAudio asks the system hot-key service for these keys, which does not need Accessibility
            permission. On a Mac laptop keyboard you may need to hold fn to send an F-key.
          </p>
        </CardContent>
      </Card>
    </>
  )
}
