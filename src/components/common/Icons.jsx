import { CH_COLORS } from '../../constants'

export function ModeIcon({ id, active }) {
  const color = active ? 'var(--color-gold)' : 'var(--color-sub)'
  if (id === 'stereo') return (
    <div className="w-9 h-9 rounded-lg bg-card border border-border/60 flex items-center justify-center shrink-0 transition-colors" style={{background: active ? 'var(--color-gold-dim)' : '', borderColor: active ? 'var(--color-gold)' : ''}}>
      <svg width="22" height="16" viewBox="0 0 22 16" fill="none">
        <rect x="0" y="7" width="3" height="2" rx="1" fill={color} opacity=".4"/>
        <rect x="4.5" y="4" width="3" height="8" rx="1.5" fill={color} opacity=".65"/>
        <rect x="9.5" y="0" width="3" height="16" rx="1.5" fill={color}/>
        <rect x="14.5" y="4" width="3" height="8" rx="1.5" fill={color} opacity=".65"/>
        <rect x="19" y="7" width="3" height="2" rx="1" fill={color} opacity=".4"/>
      </svg>
    </div>
  )
  if (id === 'keep') return (
    <div className="w-9 h-9 rounded-lg bg-card border border-border/60 flex items-center justify-center shrink-0 transition-colors" style={{background: active ? 'var(--color-gold-dim)' : '', borderColor: active ? 'var(--color-gold)' : ''}}>
      <svg width="22" height="16" viewBox="0 0 22 16" fill="none">
        <rect x="0" y="3" width="3.5" height="10" rx="1.75" fill={color} opacity=".5"/>
        <rect x="4.5" y="0" width="3.5" height="16" rx="1.75" fill={color}/>
        <rect x="9.5" y="5" width="3" height="6" rx="1.5" fill={color} opacity=".7"/>
        <rect x="14.5" y="1" width="3.5" height="14" rx="1.75" fill={color} opacity=".85"/>
        <rect x="19" y="3" width="3.5" height="10" rx="1.75" fill={color} opacity=".5"/>
      </svg>
    </div>
  )
  const cs = active ? CH_COLORS : ['var(--color-sub)','var(--color-sub)','var(--color-sub)','var(--color-sub)']
  return (
    <div className="w-9 h-9 rounded-lg bg-card border border-border/60 flex items-center justify-center shrink-0 transition-colors" style={{background: active ? 'var(--color-gold-dim)' : '', borderColor: active ? 'var(--color-gold)' : ''}}>
      <svg width="22" height="16" viewBox="0 0 22 16" fill="none">
        <rect x="0" y="3" width="4" height="10" rx="2" fill={cs[0]} opacity={active?1:.5}/>
        <rect x="5.5" y="0" width="4" height="16" rx="2" fill={cs[1]} opacity={active?1:.9}/>
        <rect x="12.5" y="3" width="4" height="10" rx="2" fill={cs[2]} opacity={active?1:.7}/>
        <rect x="18.5" y="6" width="3.5" height="4" rx="1.75" fill={cs[3]} opacity={active?1:.4}/>
      </svg>
    </div>
  )
}

export function WaveformIcon() {
  return (
    <svg width="52" height="32" viewBox="0 0 52 32" fill="none" className="mb-1">
      {[[0,14,4,4],[5,10,4,12],[10,5,4,22],[15,1,4,30],[20,4,4,24],[25,8,4,16],[30,5,4,22],[35,10,4,12],[40,13,4,6],[45,15,4,2]].map(([x,y,w,h],i) => (
        <rect key={i} x={x} y={y} width={w} height={h} rx={w/2} fill="var(--color-gold)" opacity={[.2,.35,.5,.65,.8,1,.8,.65,.45,.25][i]}/>
      ))}
    </svg>
  )
}

export function LogoSvg() {
  return (
    <svg width="32" height="32" viewBox="0 0 38 38" fill="none">
      <circle cx="19" cy="19" r="17.5" stroke="var(--color-gold)" strokeWidth="1.5" opacity=".3"/>
      <rect x="7" y="21" width="3.5" height="9" rx="1.75" fill="var(--color-gold)" opacity=".38"/>
      <rect x="12" y="14" width="3.5" height="16" rx="1.75" fill="var(--color-gold)" opacity=".65"/>
      <rect x="17" y="8" width="4" height="22" rx="2" fill="var(--color-gold)"/>
      <rect x="23" y="14" width="3.5" height="16" rx="1.75" fill="var(--color-gold)" opacity=".65"/>
      <rect x="28" y="21" width="3.5" height="9" rx="1.75" fill="var(--color-gold)" opacity=".38"/>
    </svg>
  )
}
