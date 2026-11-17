import { describe, expect, it } from "vitest";
import { chooseTheme, effectiveTheme, matchTheMac, THEME_GROUPS, THEMES } from "@/components/theme";

describe("themes", () => {
  it("lists every theme in exactly one family", () => {
    const grouped = THEME_GROUPS.flatMap((g) => g.ids).sort();
    expect(grouped).toEqual(THEMES.map((t) => t.id).sort());
  });

  it("shows the chosen theme, or the one for the Mac's appearance", () => {
    expect(effectiveTheme({ theme: "receipt" }, true)).toBe("receipt");
    const auto = { theme: "receipt" as const, autoTheme: true, lightTheme: "instrument" as const, darkTheme: "departures" as const };
    expect(effectiveTheme(auto, false)).toBe("instrument");
    expect(effectiveTheme(auto, true)).toBe("departures");
  });

  it("falls back to Ink for a theme it doesn't know", () => {
    expect(effectiveTheme({ theme: "nope" as never }, false)).toBe("ink");
  });

  it("with Match the Mac on, a pick fills the current appearance's slot", () => {
    const auto = { theme: "ink" as const, autoTheme: true, lightTheme: "paper" as const, darkTheme: "ink" as const };
    expect(chooseTheme(auto, "comic", true)).toMatchObject({ darkTheme: "comic", lightTheme: "paper" });
    expect(chooseTheme(auto, "receipt", false)).toMatchObject({ lightTheme: "receipt", darkTheme: "ink" });
    expect(chooseTheme({ theme: "ink" as const }, "bevel", true)).toEqual({ theme: "bevel" });
  });

  it("turning Match the Mac on keeps the theme in use for its own appearance", () => {
    expect(matchTheMac({ theme: "departures" as const }, true, true)).toMatchObject({ autoTheme: true, darkTheme: "departures", lightTheme: "paper" });
    expect(matchTheMac({ theme: "receipt" as const }, true, false)).toMatchObject({ autoTheme: true, lightTheme: "receipt", darkTheme: "ink" });
    const on = { theme: "ink" as const, autoTheme: true, lightTheme: "receipt" as const, darkTheme: "comic" as const };
    expect(matchTheMac(on, false, false)).toMatchObject({ autoTheme: false, theme: "receipt" });
  });
});
