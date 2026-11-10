import ServerProbes from '../../observability/servers/ServerProbes'
import CommandsPanel from '../../operations/CommandsPanel'

export default function AgentTasks({ serverId, commandsEnabled }: { serverId: number; commandsEnabled: boolean }) {
  return <><ServerProbes serverId={serverId} /><CommandsPanel serverId={serverId} enabled={commandsEnabled} /></>
}
