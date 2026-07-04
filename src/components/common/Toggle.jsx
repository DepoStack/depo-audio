import { Switch } from '../ui/switch'

export default function Toggle({ checked, onChange }) {
  return <Switch checked={checked} onCheckedChange={onChange} />
}
