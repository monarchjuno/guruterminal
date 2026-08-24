import {
  Component,
  lazy,
  Suspense,
  type ErrorInfo,
  type ReactNode,
  useEffect,
  useState,
} from "react";
import type { DiagramPlugin } from "@streamdown/mermaid";
import { hasCompleteMermaidFence, loadMermaidPlugin } from "./mermaidPlugin";

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

function useDeferredMermaidPlugin(
  text: string,
  isAnimating: boolean,
): DiagramPlugin | undefined {
  const requiresMermaid = !isAnimating && hasCompleteMermaidFence(text);
  const [plugin, setPlugin] = useState<DiagramPlugin>();

  useEffect(() => {
    let disposed = false;
    if (!requiresMermaid) {
      setPlugin(undefined);
      return () => {
        disposed = true;
      };
    }

    void loadMermaidPlugin().then(
      (loaded) => {
        if (!disposed) setPlugin(loaded);
      },
      () => {
        if (!disposed) setPlugin(undefined);
      },
    );
    return () => {
      disposed = true;
    };
  }, [requiresMermaid]);

  return requiresMermaid ? plugin : undefined;
}

export function SafeMessageResponse({ text, isAnimating, onOpenLink }: Props) {
  const fallback = <p className="message-response-fallback">{text}</p>;
  const mermaidPlugin = useDeferredMermaidPlugin(text, isAnimating);

  return (
    <MessageRenderBoundary fallback={fallback}>
      <Suspense fallback={fallback}>
        <MessageResponse
          isAnimating={isAnimating}
          mermaidPlugin={mermaidPlugin}
          onOpenLink={onOpenLink}
        >
          {text}
        </MessageResponse>
      </Suspense>
    </MessageRenderBoundary>
  );
}
