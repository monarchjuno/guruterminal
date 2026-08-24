import { SidebarTrigger } from "@/components/ui/sidebar";

export function MacTitlebarControls() {
  return (
    <div className="macos-titlebar-controls">
      <SidebarTrigger
        className="macos-titlebar-sidebar-trigger"
        aria-label="Show or hide sidebar"
        title="Show or hide sidebar (⌘B)"
      />
    </div>
  );
}
