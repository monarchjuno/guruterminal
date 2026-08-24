import {
  Maximize2Icon,
  Minimize2Icon,
  PanelBottomIcon,
  PanelRightIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import type { WorkspacePlacement } from "../../chat/workspace";

type Props = {
  placement: WorkspacePlacement;
  maximized: boolean;
  onPlacementChange: (placement: WorkspacePlacement) => void;
  onMaximizedChange: (maximized: boolean) => void;
};

export function WorkspaceLayoutControls({
  placement,
  maximized,
  onPlacementChange,
  onMaximizedChange,
}: Props) {
  return (
    <div className="workspace-header-controls">
      <Button
        type="button"
        size="icon"
        variant="ghost"
        aria-label={
          maximized ? "Restore chat workspace" : "Maximize chat workspace"
        }
        aria-pressed={maximized}
        onClick={() => onMaximizedChange(!maximized)}
      >
        {maximized ? <Minimize2Icon /> : <Maximize2Icon />}
      </Button>
      <div
        className="artifact-placement-toggle"
        role="group"
        aria-label="Workspace layout"
      >
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label="Show workspace beside chat"
          aria-pressed={placement === "side"}
          onClick={() => onPlacementChange("side")}
        >
          <PanelRightIcon />
        </Button>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          aria-label="Show workspace below chat"
          aria-pressed={placement === "bottom"}
          onClick={() => onPlacementChange("bottom")}
        >
          <PanelBottomIcon />
        </Button>
      </div>
    </div>
  );
}
