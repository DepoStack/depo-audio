import { useId, useState } from 'react'
import { Volume2 } from 'lucide-react'
import { cn } from '../../lib/utils'
import {
  CHANNEL_VOLUME_MAX,
  LISTENING_ONLY_NOTE,
  MASTER_VOLUME_STEP,
  audibleChannels,
  hearingSummary,
  initialStrips,
  separationNote,
  stripChanged,
  stripLabels,
  volumePercent,
} from '../../lib/channelMixer'

// Every control here is at least 44px high (WCAG 2.5.5, AAA target size).
const toggleClass = pressed =>
  cn(
    'h-11 rounded-md border px-3 type-small transition-colors',
    pressed
      ? 'border-foreground bg-secondary text-foreground ring-1 ring-inset ring-foreground font-semibold'
      : 'border-border text-[hsl(var(--text2))] hover:text-foreground hover:bg-secondary',
  )
// A 44px-high range input: the track stays thin, but the whole row takes the
// pointer.
// mix-range (index.css) draws the track with a border of at least 3:1
// against the card in both themes (WCAG 1.4.11).
const sliderClass = 'mix-range h-11 min-w-0 flex-1 cursor-pointer'
const fillStyle = (value, max) => ({ '--mix-fill': `${Math.min(100, Math.max(0, (Number(value) / max) * 100) || 0)}%` })

// Note shown with Separate channels on when the track has no channels to
// separate (or its channels could not be counted). The toggle stays so the
// person can turn the mix off; nothing turns it off for them.
function noStripsNote({ fileChannels, probed }) {
  if (!probed) return ''
  if (fileChannels === 1) {
    return 'This track has 1 channel, so there is nothing to separate. Turn off Separate channels to go back to normal playback.'
  }
  if (!fileChannels) {
    return "DepoAudio could not count this track's channels, so there are no strips. Turn off Separate channels to go back to normal playback."
  }
  return ''
}

// Master volume for the Player plus, for a file with 2 or more channels, an
// opt-in strip per channel (Mute, Solo, volume). Everything here changes what
// the person hears only.
export default function ListeningMix({
  volume,
  onVolume,
  fileChannels,
  mixOn,
  onToggleMix,
  strips,
  onStrips,
  heardChannels,
  probeAvailable,
  probed = true,
  stripsCleared = false,
  stripsFromPreviousCleared = false,
  notice = '',
}) {
  const noteId = useId()
  const separateNoteId = useId()
  // The strips Reset channels produced. While they are still the strips in
  // use, the status line confirms the reset; any change (or a new track)
  // replaces them.
  const [resetStrips, setResetStrips] = useState(null)
  const justReset = resetStrips != null && resetStrips === strips
  const labels = stripLabels({ fileChannels, heardChannels })
  const shown = strips.slice(0, labels.length)
  const audible = audibleChannels(shown)
  const anySolo = shown.some(s => s.soloed)
  const anyChanged = shown.some(stripChanged)
  const showStrips = mixOn && shown.length > 0
  // Up to 4 channels: Mute, Solo and volume on one line per strip. More: a
  // narrower strip so 4 fit across and 16 channels take 4 rows.
  const wide = shown.length <= 4
  const note = mixOn
    ? noStripsNote({ fileChannels, probed }) || separationNote({ fileChannels, heardChannels, probeAvailable })
    : ''
  const status = !showStrips
    ? ''
    : hearingSummary(shown) ||
      (justReset
        ? 'Channels reset. Every channel is heard at 100%.'
        : stripsCleared
          ? 'The web view mixes this file down, so the Mute, Solo and volume settings you made for its channels were cleared.'
          : stripsFromPreviousCleared
            ? 'New track: the Mute, Solo and volume settings from the previous track were cleared. Every channel is heard at 100%.'
            : '')
  const update = (i, patch) => onStrips(strips.map((s, j) => (j === i ? { ...s, ...patch } : s)))
  const reset = () => {
    // aria-disabled keeps the button (and keyboard focus) in place when
    // there is nothing to reset.
    if (!anyChanged) return
    const next = initialStrips(strips.length)
    setResetStrips(next)
    onStrips(next)
  }
  const toggleTitle = fileChannels >= 2 ? `Hear each of the ${fileChannels} channels on its own. ` : ''

  return (
    <section aria-label="Listening mix" className="flex flex-col gap-2 border-t border-border pt-3">
      <p id={noteId} className="type-small text-[hsl(var(--sub))]">
        {LISTENING_ONLY_NOTE}
      </p>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <label className="flex min-w-[12rem] flex-1 items-center gap-2 type-small text-[hsl(var(--text2))]">
          <Volume2 size={14} aria-hidden="true" className="shrink-0" />
          <span className="shrink-0">Volume</span>
          <input
            type="range"
            min="0"
            max="1"
            step={MASTER_VOLUME_STEP}
            value={volume}
            aria-label="Volume"
            aria-describedby={noteId}
            aria-valuetext={volumePercent(volume)}
            className={sliderClass}
            style={fillStyle(volume, 1)}
            onChange={e => onVolume(Number(e.target.value))}
          />
          <span className="w-10 shrink-0 text-right tabular-nums text-foreground">{volumePercent(volume)}</span>
        </label>
        {(fileChannels >= 2 || mixOn) && (
          <button
            type="button"
            aria-pressed={mixOn}
            aria-describedby={separateNoteId}
            className={toggleClass(mixOn)}
            title={`${toggleTitle}${LISTENING_ONLY_NOTE}`}
            onClick={onToggleMix}
          >
            Separate channels
          </button>
        )}
        {showStrips && (
          <button
            type="button"
            aria-disabled={!anyChanged}
            className={cn(
              'h-11 rounded-md px-3 type-small underline underline-offset-2',
              anyChanged
                ? 'text-[hsl(var(--text2))] hover:text-foreground hover:bg-secondary'
                : 'cursor-default text-[hsl(var(--sub))] no-underline',
            )}
            onClick={reset}
          >
            Reset channels
          </button>
        )}
        <span id={separateNoteId} className="sr-only">
          {`Mute, solo or turn down each of this file's ${fileChannels || 0} channels. ${LISTENING_ONLY_NOTE}`}
        </span>
      </div>

      {/* Messages sit above the strips so they stay in view with many channels. */}
      <p role="status" aria-live="polite" className="type-small text-foreground">
        {status}
      </p>
      {note && <p className="type-small text-foreground">{note}</p>}
      <p role="alert" className="type-small text-foreground empty:hidden">
        {notice}
      </p>

      {showStrips && (
        <div
          role="group"
          aria-label="Channel strips"
          aria-describedby={noteId}
          className={cn(
            'grid gap-2',
            wide ? 'grid-cols-[repeat(auto-fill,minmax(17rem,1fr))]' : 'grid-cols-[repeat(auto-fill,minmax(8rem,1fr))]',
          )}
        >
          {shown.map((strip, i) => {
            const label = labels[i]
            const state = audible[i] ? '' : strip.muted ? 'Muted' : anySolo ? 'Silent (solo)' : ''
            return (
              <div
                key={label}
                className="flex min-w-0 flex-col gap-1 rounded-md border border-border p-2"
                data-testid="channel-strip"
              >
                <div className="flex flex-wrap items-baseline justify-between gap-x-2">
                  <span className="type-small font-semibold text-foreground">{label}</span>
                  <span className="type-small text-[hsl(var(--sub))]">{state}</span>
                </div>
                <div className={cn('gap-1', wide ? 'flex items-center' : 'grid grid-cols-2')}>
                  <button
                    type="button"
                    aria-pressed={strip.muted}
                    aria-label={`Mute ${label}`}
                    className={cn(toggleClass(strip.muted), 'px-1', wide && 'w-14 shrink-0')}
                    onClick={() => update(i, { muted: !strip.muted })}
                  >
                    Mute
                  </button>
                  <button
                    type="button"
                    aria-pressed={strip.soloed}
                    aria-label={`Solo ${label}`}
                    className={cn(toggleClass(strip.soloed), 'px-1', wide && 'w-14 shrink-0')}
                    onClick={() => update(i, { soloed: !strip.soloed })}
                  >
                    Solo
                  </button>
                  {wide && <StripVolume label={label} strip={strip} onChange={v => update(i, { volume: v })} />}
                </div>
                {!wide && <StripVolume label={label} strip={strip} onChange={v => update(i, { volume: v })} />}
              </div>
            )
          })}
        </div>
      )}
    </section>
  )
}

function StripVolume({ label, strip, onChange }) {
  return (
    <div className="flex min-w-0 flex-1 items-center gap-1">
      <input
        type="range"
        min="0"
        max={CHANNEL_VOLUME_MAX}
        step="0.05"
        value={strip.volume}
        aria-label={`${label} volume`}
        aria-valuetext={volumePercent(strip.volume)}
        className={sliderClass}
        style={fillStyle(strip.volume, CHANNEL_VOLUME_MAX)}
        onChange={e => onChange(Number(e.target.value))}
      />
      <span className="w-10 shrink-0 text-right type-small tabular-nums text-foreground">
        {volumePercent(strip.volume)}
      </span>
    </div>
  )
}
