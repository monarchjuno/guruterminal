import { errorMessage } from "./errors";

describe("errorMessage", () => {
  it("preserves Error, string, and serialized Tauri command messages", () => {
    expect(errorMessage(new Error("native error"), "fallback")).toBe(
      "native error",
    );
    expect(errorMessage("string error", "fallback")).toBe("string error");
    expect(
      errorMessage({ code: "internal", message: "Tauri error" }, "fallback"),
    ).toBe("Tauri error");
  });

  it("uses the fallback for empty or unknown values", () => {
    expect(errorMessage({ message: "" }, "fallback")).toBe("fallback");
    expect(errorMessage(null, "fallback")).toBe("fallback");
  });
});
