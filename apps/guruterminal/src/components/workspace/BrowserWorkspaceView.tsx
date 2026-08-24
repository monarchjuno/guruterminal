import type { FormEventHandler, RefObject } from "react";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  ExternalLinkIcon,
  GlobeIcon,
  RefreshCwIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import type { BrowserWorkspaceTab } from "../../chat/workspace";

type Props = {
  tab: BrowserWorkspaceTab;
  address: string;
  viewportRef: RefObject<HTMLDivElement | null>;
  onAddressChange: (address: string) => void;
  onNavigate: FormEventHandler<HTMLFormElement>;
  onAction: (action: "back" | "forward" | "reload") => void;
  onOpenExternal: (url: string) => void;
};

export function BrowserWorkspaceView({
  tab,
  address,
  viewportRef,
  onAddressChange,
  onNavigate,
  onAction,
  onOpenExternal,
}: Props) {
  return (
    <>
      <form className="browser-toolbar" onSubmit={onNavigate}>
        <div className="browser-history-actions">
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="Go back"
            disabled={!tab.native_id}
            onClick={() => onAction("back")}
          >
            <ArrowLeftIcon />
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="Go forward"
            disabled={!tab.native_id}
            onClick={() => onAction("forward")}
          >
            <ArrowRightIcon />
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="Reload page"
            disabled={!tab.native_id}
            onClick={() => onAction("reload")}
          >
            <RefreshCwIcon />
          </Button>
        </div>
        <input
          aria-label="Web address"
          value={address}
          placeholder="https://example.com"
          spellCheck={false}
          onChange={(event) => onAddressChange(event.target.value)}
        />
        <Button type="submit" size="sm">
          Open
        </Button>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label="Open in system browser"
          disabled={!tab.url}
          onClick={() => onOpenExternal(tab.url)}
        >
          <ExternalLinkIcon />
        </Button>
      </form>
      {tab.error && (
        <div className="browser-error" role="alert">
          {tab.error}
        </div>
      )}
      {!tab.url ? (
        <div className="browser-start">
          <GlobeIcon />
          <h2>Browse in Guru Terminal</h2>
          <p>
            Enter an HTTP or HTTPS address above. Search queries are not sent
            to a search engine.
          </p>
        </div>
      ) : (
        <div className="browser-viewport" ref={viewportRef} aria-label="Browser content">
          {tab.loading && (
            <span className="browser-loading">
              <Spinner /> Loading page…
            </span>
          )}
        </div>
      )}
    </>
  );
}
