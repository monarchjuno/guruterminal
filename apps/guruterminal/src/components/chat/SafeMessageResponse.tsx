import {
  Component,
  lazy,
  Suspense,
  type ErrorInfo,
  type ReactNode,
} from "react";

const MessageResponse = lazy(() =>
  import("@/components/ai-elements/message-response").then((module) => ({
    default: module.MessageResponse,
  })),
);

type BoundaryProps = {
  children: ReactNode;
  fallback: ReactNode;
};

type BoundaryState = {
  failed: boolean;
};

export class MessageRenderBoundary extends Component<
  BoundaryProps,
  BoundaryState
> {
  state: BoundaryState = { failed: false };

  static getDerivedStateFromError(): BoundaryState {
    return { failed: true };
  }

  componentDidCatch(_error: Error, _info: ErrorInfo) {
    // The readable response projection remains visible below. Do not log
    // response content or renderer internals from this recovery boundary.
  }

  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

type Props = {
  text: string;
  isAnimating: boolean;
  onOpenLink: (url: string) => Promise<void> | void;
};

export function SafeMessageResponse({ text, isAnimating, onOpenLink }: Props) {
  const fallback = <p className="message-response-fallback">{text}</p>;

  return (
    <MessageRenderBoundary fallback={fallback}>
      <Suspense fallback={fallback}>
        <MessageResponse isAnimating={isAnimating} onOpenLink={onOpenLink}>
          {text}
        </MessageResponse>
      </Suspense>
    </MessageRenderBoundary>
  );
}
