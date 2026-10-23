import ServerProbes from './ServerProbes'
import CommandsPanel from './CommandsPanel'

export default function AgentTasks({ serverId, commandsEnabled }: { serverId: number; commandsEnabled: boolean }) {
  return <><ServerProbes serverId={serverId} /><CommandsPanel serverId={serverId} enabled={commandsEnabled} /></>
}
