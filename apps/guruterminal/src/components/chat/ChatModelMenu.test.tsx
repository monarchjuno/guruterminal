import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChatModelMenu } from "./ChatModelMenu";

describe("ChatModelMenu", () => {
  it("lets the user recover an empty thinking selection from Pi's model levels", async () => {
    const user = userEvent.setup();
    const onSelectionChange = vi.fn();

    render(
      <ChatModelMenu
        models={[
          {
            id: "openai-codex/gpt-5.6-sol",
            name: "GPT-5.6 Sol",
            provider: "openai-codex",
            model: "gpt-5.6-sol",
            input: ["text"],
            reasoning: true,
            context_window: 272_000,
            max_tokens: 128_000,
            thinking_levels: ["off", "medium", "high", "max"],
            thinking_level_map: { max: "max" },
            run_controls: [],
            credential_source: "saved",
          },
        ]}
        providers={[
          {
            id: "openai-codex",
            label: "OpenAI with ChatGPT",
            description: "OpenAI models",
            api_key: false,
            oauth: { label: "Continue with ChatGPT" },
            credential_label: "ChatGPT",
            credential_source: "saved",
            recommended: true,
          },
        ]}
        selection={{
          model_profile_id: "openai-codex/gpt-5.6-sol",
          thinking_level: "",
          run_options: {},
        }}
        onSelectionChange={onSelectionChange}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "Model settings for this message" }),
    );
    await user.click(screen.getByRole("menuitemradio", { name: "max" }));
    expect(onSelectionChange).toHaveBeenCalledWith({
      model_profile_id: "openai-codex/gpt-5.6-sol",
      thinking_level: "max",
      run_options: {},
    });
  });
});
