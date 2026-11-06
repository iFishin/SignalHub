import { describe, expect, it } from "vitest";
import { checkAgentDraft, EMPTY_AGENT_DRAFT, type AgentDraft } from "./customAgentForm";

const families = ["claude-code", "codex", "cursor"];
const valid: AgentDraft = {
  id: "My_Agent",
  name: "My Agent",
  family: "claude-code",
  config: ".my-agent/settings.json",
  marker: ".my-agent",
  command: "my-agent",
};

describe("checkAgentDraft", () => {
  it("填全了就放行，不因为大写 id 或下划线误报", () => {
    // normalize_id 在 Rust 侧；前端刻意不判 id 的字符与长度。
    expect(checkAgentDraft(valid, families)).toBeNull();
  });

  it("四个必填字段逐个挡住", () => {
    expect(checkAgentDraft({ ...valid, id: "  " }, families)).toBe("idRequired");
    expect(checkAgentDraft({ ...valid, name: "" }, families)).toBe("nameRequired");
    expect(checkAgentDraft({ ...valid, family: "" }, families)).toBe("familyRequired");
    expect(checkAgentDraft({ ...valid, config: " " }, families)).toBe("configRequired");
  });

  it("family 只用运行时给出的清单，不认自己编的名字", () => {
    expect(checkAgentDraft({ ...valid, family: "not-a-family" }, families)).toBe("familyRequired");
    expect(checkAgentDraft(valid, ["someone-else"])).toBe("familyRequired");
  });

  it("清单还没从 IPC 回来时不拦，交给 Rust 报准话", () => {
    expect(checkAgentDraft(valid, [])).toBeNull();
  });

  it("拒绝 config / marker 里的 .. 路径段，但放过正常文件名", () => {
    expect(checkAgentDraft({ ...valid, config: "../other/settings.json" }, families)).toBe("configParent");
    expect(checkAgentDraft({ ...valid, config: ".agent/../settings.json" }, families)).toBe("configParent");
    expect(checkAgentDraft({ ...valid, config: "C:\\agent\\..\\settings.json" }, families)).toBe("configParent");
    expect(checkAgentDraft({ ...valid, marker: "..\\agent" }, families)).toBe("configParent");
    expect(checkAgentDraft({ ...valid, config: ".agent/my..backup.json" }, families)).toBeNull();
  });

  it("command 只能是命令名，带参数或空格不放行", () => {
    expect(checkAgentDraft({ ...valid, command: "my-agent --yes" }, families)).toBe("commandSpaces");
    expect(checkAgentDraft({ ...valid, command: "  " }, families)).toBeNull();
  });

  it("空草稿逐项报错的顺序稳定", () => {
    expect(checkAgentDraft(EMPTY_AGENT_DRAFT, families)).toBe("idRequired");
  });
});
