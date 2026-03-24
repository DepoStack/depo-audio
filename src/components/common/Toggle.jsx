import * as Switch from '@radix-ui/react-switch'

export default function Toggle({ checked, onChange }) {
  return (
    <Switch.Root
      className="toggle"
      checked={checked}
      onCheckedChange={onChange}
    >
      <Switch.Thumb className="toggle-thumb" />
    </Switch.Root>
  )
}
