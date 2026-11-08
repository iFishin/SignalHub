import { describe, expect, it } from "vitest";
import {
  filterBySource,
  fullLink,
  lastSeqOf,
  newestFirst,
  normalizeBase,
  openableUrl,
  unreadBySource,
  unreadCount,
  type CallMeMessage,
  type CallMeSource,
} from "./callme";

function message(seq: number, sourceId: string): CallMeMessage {
  return {
    seq,
    id: `id${seq}`,
    source_id: sourceId,
    received_ms: seq,
    title: null,
    text: `第 ${seq} 条`,
    agent: null,
    context: null,
  };
}

const source: CallMeSource = {
  id: "abc12345",
  label: "博客",
  note: null,
  token: "0123456789abcdef0123456789abcdef",
  link: "/c/0123456789abcdef0123456789abcdef",
  created_ms: 0,
  revoked: false,
  total: 0,
  today: 0,
  last_received_ms: 0,
};

describe("normalizeBase", () => {
  it("补 https，去掉尾斜杠", () => {
    expect(normalizeBase("callme.example.com")).toBe("https://callme.example.com");
    expect(normalizeBase("https://callme.example.com/")).toBe(
      "https://callme.example.com",
    );
  });

  it("明确写出的 http 不升级（本地调试）", () => {
    expect(normalizeBase("http://127.0.0.1:8080/")).toBe("http://127.0.0.1:8080");
  });

  it("误粘的接口路径会被去掉", () => {
    expect(normalizeBase("  https://callme.example.com/v1/callme  ")).toBe(
      "https://callme.example.com",
    );
  });
});

describe("fullLink", () => {
  it("拼接域名与相对路径", () => {
    expect(fullLink("https://callme.example.com", source)).toBe(
      "https://callme.example.com/c/0123456789abcdef0123456789abcdef",
    );
  });

  it("相对路径缺斜杠也能拼对", () => {
    expect(fullLink("callme.example.com/", { ...source, link: "c/x" })).toBe(
      "https://callme.example.com/c/x",
    );
  });
});

describe("未读", () => {
  const messages = [message(1, "a"), message(2, "a"), message(3, "b")];

  it("没有已读记录时全部未读", () => {
    expect(unreadCount(messages, {})).toBe(3);
  });

  it("按来源各自的游标计算", () => {
    expect(unreadCount(messages, { a: 1 })).toBe(2);
    expect(unreadCount(messages, { a: 2, b: 3 })).toBe(0);
  });

  it("可以只看某个来源", () => {
    expect(unreadCount(messages, {}, "a")).toBe(2);
    expect(unreadCount(messages, { a: 2 }, "a")).toBe(0);
  });

  it("按来源分组计数", () => {
    expect(unreadBySource(messages, { a: 1 })).toEqual({ a: 1, b: 1 });
    expect(unreadBySource(messages, { a: 2, b: 3 })).toEqual({});
  });
});

describe("列表辅助", () => {
  const messages = [message(1, "a"), message(2, "b"), message(3, "a"), message(4, "b")];

  it("lastSeqOf 取该来源最大值（没有就是 0）", () => {
    expect(lastSeqOf(messages, "a")).toBe(3);
    expect(lastSeqOf(messages, "b")).toBe(4);
    expect(lastSeqOf(messages, "zzz")).toBe(0);
  });

  it("按来源过滤", () => {
    expect(filterBySource(messages, null)).toHaveLength(4);
    expect(filterBySource(messages, "a").map((m) => m.seq)).toEqual([1, 3]);
  });

  it("展示顺序是新→旧", () => {
    expect(newestFirst(messages).map((m) => m.seq)).toEqual([4, 3, 2, 1]);
    // 不改原数组
    expect(messages[0].seq).toBe(1);
  });
});

describe("openableUrl", () => {
  it("只放行 http/https", () => {
    expect(openableUrl("https://blog.example.com/a")).toBe(
      "https://blog.example.com/a",
    );
    expect(openableUrl("  http://x.test/  ")).toBe("http://x.test/");
    expect(openableUrl("file:///etc/passwd")).toBeNull();
    expect(openableUrl("javascript:alert(1)")).toBeNull();
    expect(openableUrl(null)).toBeNull();
    expect(openableUrl("我在某篇文章的页脚读到的")).toBeNull();
  });
});
