import { lazy, Suspense, useEffect, useState } from "react";
import { CheckIcon, ClipboardIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { SafeMessageResponse } from "@/components/chat/SafeMessageResponse";
import type { ChatArtifactRef, ChatArtifactView } from "../../types";

const ArtifactChart = lazy(() => import("./ArtifactChart"));

type Props = {
  view: ChatArtifactView;
  theme: "light" | "dark";
  onOpenLink: (url: string) => void;
};

export const chatArtifactRef = (view: ChatArtifactView): ChatArtifactRef => ({
  artifact_id: view.artifact.id,
  revision: view.revision.revision,
  kind: view.artifact.kind,
  title: view.artifact.title,
  digest: view.revision.digest,
});

export function ArtifactContent({
  view,
  theme,
  onOpenLink,
}: Props) {
  const [mode, setMode] = useState<"preview" | "source">("preview");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    setMode("preview");
    setCopied(false);
  }, [view.artifact.id, view.revision.revision]);

  const copySource = async () => {
    if (view.revision.payload.kind !== "markdown") return;
    await navigator.clipboard.writeText(view.revision.payload.markdown);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1_500);
  };

  const isMarkdown = view.artifact.kind === "markdown";

  return (
    <div className="workspace-artifact-view">
      {isMarkdown ? (
        <div className="artifact-view-toolbar">
          <div className="artifact-view-leading">
            <div className="artifact-view-tabs" role="tablist">
              <button
                type="button"
                role="tab"
                aria-selected={mode === "preview"}
                onClick={() => setMode("preview")}
              >
                Preview
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={mode === "source"}
                onClick={() => setMode("source")}
              >
                Source
              </button>
            </div>
          </div>
          <div className="artifact-toolbar-actions">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => void copySource()}
            >
              {copied ? <CheckIcon /> : <ClipboardIcon />}
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
        </div>
      ) : null}
      <div className="artifact-view-content">
        {mode === "source" && view.revision.payload.kind === "markdown" ? (
          <pre className="artifact-source">
            <code>{view.revision.payload.markdown}</code>
          </pre>
        ) : view.revision.payload.kind === "markdown" ? (
          <article className="artifact-markdown">
            <SafeMessageResponse
              text={view.revision.payload.markdown}
              isAnimating={false}
              onOpenLink={onOpenLink}
            />
          </article>
        ) : view.chart_dataset ? (
          <Suspense
            fallback={
              <div className="artifact-panel-state" role="status">
                <Spinner />
                <span>Loading financial chart…</span>
              </div>
            }
          >
            <ArtifactChart
              chart={view.revision.payload.chart}
              dataset={view.chart_dataset}
              theme={theme}
            />
          </Suspense>
        ) : (
          <div className="artifact-panel-state" role="alert">
            Chart data is unavailable.
          </div>
        )}
      </div>
      <footer className="artifact-view-footer">
        <span>
          {view.artifact.kind === "chart" ? "Chart" : "Document"}
        </span>
        <time dateTime={new Date(view.artifact.updated_at_ms).toISOString()}>
          Updated{" "}
          {new Intl.DateTimeFormat("en-US", {
            month: "short",
            day: "numeric",
            hour: "numeric",
            minute: "2-digit",
          }).format(view.artifact.updated_at_ms)}
        </time>
      </footer>
    </div>
  );
}
