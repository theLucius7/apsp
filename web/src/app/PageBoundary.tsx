import { Component } from 'react'
import type { ReactNode } from 'react'
import { CircleAlertIcon } from 'lucide-react'
import { isChunkLoadError, reloadForNewRelease } from '@/lib/new-release'
import { Button } from '@/components/ui/button'
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'

type Props = { resetKey: string; children: ReactNode }
type State = { error: Error | null }

/**
 * Keeps a failure inside the page that caused it: the shell, navigation and every
 * other page stay usable. Going to another address clears the failure.
 */
export default class PageBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) }
  }

  componentDidCatch(error: Error) {
    if (isChunkLoadError(error)) void reloadForNewRelease()
  }

  componentDidUpdate(previous: Props) {
    if (previous.resetKey !== this.props.resetKey && this.state.error) this.setState({ error: null })
  }

  render() {
    if (!this.state.error) return this.props.children
    return <div data-slot="page-failure"><Empty className="py-16">
      <EmptyHeader>
        <EmptyMedia variant="icon"><CircleAlertIcon aria-hidden="true" /></EmptyMedia>
        <EmptyTitle><h2>这个页面显示时出错了</h2></EmptyTitle>
        <EmptyDescription>其他页面不受影响。可以重试，或从导航进入其他页面；问题持续时刷新浏览器。</EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <code className="text-muted-foreground max-w-md text-xs wrap-anywhere">{this.state.error.message}</code>
        <Button variant="outline" onClick={() => this.setState({ error: null })}>重试</Button>
      </EmptyContent>
    </Empty></div>
  }
}
