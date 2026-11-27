import { describe, expect, it } from "vitest";
import {
  EMPTY_DOC,
  addItem,
  addMember,
  doneItems,
  dueLabel,
  fromLocalInput,
  groupByAssignee,
  markAlerted,
  overdueCount,
  parseDoc,
  postponed,
  removeItem,
  removeMember,
  setDone,
  toLocalInput,
  unalerted,
  updateItem,
} from "./delegations";

const H = 3_600_000;
const NOW = 1_800_000_000_000;

function team() {
  return addMember(addMember(EMPTY_DOC, "Asha"), "Ben"); // ids 1 and 2
}

describe("the team", () => {
  it("trims, refuses blanks and duplicates (any case)", () => {
    expect(team().team.map((m) => m.name)).toEqual(["Asha", "Ben"]);
    expect(() => addMember(team(), "  ")).toThrow();
    expect(() => addMember(team(), "asha")).toThrow("already");
  });
  it("cannot remove someone who still has work", () => {
    const doc = addItem(team(), { title: "x", assigneeId: 1, dueAt: NOW }, NOW);
    expect(() => removeMember(doc, 1)).toThrow("reassign");
    expect(removeMember(doc, 2).team).toHaveLength(1);
  });
});

describe("a delegation", () => {
  it("needs a title, a team member and a follow-up time", () => {
    const t = team();
    expect(() => addItem(t, { title: " ", assigneeId: 1, dueAt: NOW }, NOW)).toThrow("title");
    expect(() => addItem(t, { title: "x", assigneeId: 9, dueAt: NOW }, NOW)).toThrow("who");
    expect(() => addItem(t, { title: "x", assigneeId: 1, dueAt: NaN }, NOW)).toThrow("follow-up time");
  });
  it("cannot lose its time on update, and ids are not reused after finishing", () => {
    const doc = addItem(team(), { title: "x", assigneeId: 1, dueAt: NOW }, NOW);
    const id = doc.items[0].id;
    expect(() => updateItem(doc, id, { dueAt: NaN })).toThrow("follow-up time");
    const gone = removeItem(doc, id);
    expect(addItem(gone, { title: "y", assigneeId: 1, dueAt: NOW }, NOW).items[0].id).not.toBe(id);
  });
});

describe("grouping", () => {
  it("leaves out people without work and puts the most urgent person first", () => {
    let doc = addMember(team(), "Cy");
    doc = addItem(doc, { title: "late", assigneeId: 2, dueAt: NOW + 5 * H }, NOW);
    doc = addItem(doc, { title: "soon", assigneeId: 2, dueAt: NOW + H }, NOW);
    doc = addItem(doc, { title: "other", assigneeId: 1, dueAt: NOW + 2 * H }, NOW);
    const g = groupByAssignee(doc);
    expect(g.map((x) => x.member.name)).toEqual(["Ben", "Asha"]);
    expect(g[0].items.map((d) => d.title)).toEqual(["soon", "late"]);
  });
});

describe("overdue and alerts", () => {
  it("counts items at or past the deadline", () => {
    let doc = addItem(team(), { title: "a", assigneeId: 1, dueAt: NOW - H }, NOW);
    doc = addItem(doc, { title: "b", assigneeId: 1, dueAt: NOW + H }, NOW);
    expect(overdueCount(doc.items, NOW)).toBe(1);
  });
  it("alerts once per deadline, and again after a postpone", () => {
    let doc = addItem(team(), { title: "a", assigneeId: 1, dueAt: NOW - H }, NOW);
    expect(unalerted(doc.items, NOW)).toHaveLength(1);
    doc = markAlerted(doc, [doc.items[0].id]);
    expect(unalerted(doc.items, NOW)).toHaveLength(0);
    doc = updateItem(doc, doc.items[0].id, { dueAt: NOW + H });
    expect(unalerted(doc.items, NOW + 2 * H)).toHaveLength(1);
  });
});

describe("done work", () => {
  const open = () => addItem(addItem(team(), { title: "a", assigneeId: 1, dueAt: NOW - H }, NOW), { title: "b", assigneeId: 2, dueAt: NOW - H }, NOW);
  it("leaves the open list, stops counting and alerting, and keeps its notes", () => {
    let doc = open();
    doc = updateItem(doc, 1 + 2, { notes: "agreed Friday" }); // ids 1,2 are the team, so the items are 3 and 4
    doc = setDone(doc, 3, NOW);
    expect(groupByAssignee(doc).map((g) => g.member.name)).toEqual(["Ben"]);
    expect(overdueCount(doc.items, NOW)).toBe(1);
    expect(unalerted(doc.items, NOW).map((d) => d.id)).toEqual([4]);
    expect(doneItems(doc).map((d) => d.notes)).toEqual(["agreed Friday"]);
  });
  it("comes back when reopened", () => {
    const doc = setDone(setDone(open(), 3, NOW), 3, null);
    expect(groupByAssignee(doc)).toHaveLength(2);
    expect(doneItems(doc)).toHaveLength(0);
    expect(() => setDone(doc, 99, NOW)).toThrow("not found");
  });
  it("lists the latest finished first", () => {
    const doc = setDone(setDone(open(), 3, NOW), 4, NOW + H);
    expect(doneItems(doc).map((d) => d.id)).toEqual([4, 3]);
  });
  it("lets a person go once only their done work is left, and takes it with them", () => {
    const doc = setDone(open(), 3, NOW);
    expect(() => removeMember(open(), 1)).toThrow("reassign");
    const gone = removeMember(doc, 1);
    expect(gone.items.map((d) => d.id)).toEqual([4]);
  });
});

describe("times", () => {
  it("postpones from the old deadline, or from now when it has passed", () => {
    expect(postponed(NOW + H, NOW, 1)).toBe(NOW + H + 24 * H);
    expect(postponed(NOW - 5 * H, NOW, 1)).toBe(NOW + 24 * H);
  });
  it("labels the deadline", () => {
    expect(dueLabel(NOW - 3 * H, NOW)).toBe("overdue 3h");
    expect(dueLabel(NOW + 2 * H, NOW)).toBe("in 2h");
    expect(dueLabel(NOW, NOW)).toBe("due now");
  });
  it("round trips a datetime-local value and rejects an empty one", () => {
    const ms = new Date(2026, 10, 27, 9, 30).getTime();
    expect(toLocalInput(ms)).toBe("2026-11-27T09:30");
    expect(fromLocalInput("2026-11-27T09:30")).toBe(ms);
    expect(fromLocalInput("")).toBeNaN();
  });
});

describe("parseDoc", () => {
  it("starts empty, and drops entries that point at nobody", () => {
    expect(parseDoc("")).toEqual(EMPTY_DOC);
    const text = JSON.stringify({
      nextId: 1,
      team: [{ id: 4, name: "Asha" }],
      items: [
        { id: 5, title: "ok", assigneeId: 4, dueAt: NOW, notes: "", alertedFor: null, createdAt: NOW },
        { id: 6, title: "orphan", assigneeId: 99, dueAt: NOW, notes: "", alertedFor: null, createdAt: NOW },
      ],
    });
    const doc = parseDoc(text);
    expect(doc.items.map((d) => d.title)).toEqual(["ok"]);
    expect(doc.items[0].doneAt).toBeNull(); // saved before done work was kept
    expect(doc.nextId).toBe(6); // never reuses an id already in the file
  });
});
