import { expect, test, type Page } from "@playwright/test";
import { createRequire } from "node:module";
import { installMockTauri } from "./mock-tauri";

const {version} = createRequire(import.meta.url)("../../package.json") as {version: string};
const home = (page: Page) => page.getByTestId("home-page");
const homeHeader = (page: Page) => home(page).locator("header");
const homeAction = (page: Page, name: string) => homeHeader(page).getByRole("button", {name: name === "开始使用" ? name : `前往${name}`, exact: true});
const sidebar = (page: Page) => page.getByRole("navigation", {name: "主导航", exact: true});

async function returnHome(page: Page) {
  await sidebar(page).getByRole("button", {name: "首页", exact: true}).click();
  await expect(home(page)).toBeVisible();
}

async function expectHomeActions(page: Page) {
  const actions = ["开始使用", "游戏库", "游览", "设置"].map(name => homeAction(page, name));
  for (const action of actions) await expect(action).toBeVisible();
  const inOrder = await homeHeader(page).evaluate((header, names) => {
    const buttons = names.map(name => [...header.querySelectorAll("button")]
      .find(button => button.textContent?.trim() === name));
    return buttons.every((button): button is HTMLButtonElement => !!button)
      && buttons.slice(0, -1).every((button, index) =>
        !!(button.compareDocumentPosition(buttons[index + 1]!) & Node.DOCUMENT_POSITION_FOLLOWING));
  }, ["开始使用", "游戏库", "游览", "设置"]);
  expect(inOrder).toBe(true);
}

async function verifyNavigation(page: Page) {
  await homeAction(page, "开始使用").click();
  await expect(page.getByRole("textbox", {name: "告诉 Agent 你想做什么"})).toBeVisible();
  await returnHome(page);

  await homeAction(page, "游戏库").click();
  await expect(page.getByTestId("library-page")).toBeVisible();
  await expect(sidebar(page).getByRole("button", {name: "游戏库", exact: true})).toHaveAttribute("aria-current", "page");
  await returnHome(page);

  await homeAction(page, "游览").click();
  await expect(sidebar(page).getByRole("button", {name: "游览", exact: true})).toHaveAttribute("aria-current", "page");
  await returnHome(page);

  await homeAction(page, "设置").click();
  await expect(page.getByRole("heading", {name: "设置", exact: true})).toBeVisible();
  await returnHome(page);
}

async function expectSponsorBlock(page: Page, names: string[]) {
  const main = home(page);
  const sponsorHeading = main.getByRole("heading", {name: "赞助商", exact: true});
  await expect(sponsorHeading).toHaveCount(1);
  await expect(main.getByRole("heading", {name: "赞助我", exact: true})).toHaveCount(0);
  const card = main.locator('[data-slot="card"]').filter({has: page.getByRole("heading", {name: "赞助商", exact: true})});
  const list = card.getByRole("list", {name: "赞助商", exact: true});
  for (const name of names) await expect(list.getByRole("listitem").filter({hasText: name})).toBeVisible();

  const sponsor = card.getByRole("link", {name: "去赞助", exact: true});
  await expect(sponsor).toHaveAttribute("href", "https://sponsor.aoe.top/");
  await expect(sponsor).toHaveAttribute("target", "_blank");
  const order = await card.evaluate((element) => {
    const title = element.querySelector('[data-slot="card-title"]');
    const roster = element.querySelector('ul[aria-label="赞助商"]');
    const link = [...element.querySelectorAll("a")].find(anchor => anchor.textContent?.includes("去赞助"));
    return !!title && !!roster && !!link
      && !!(title.compareDocumentPosition(roster) & Node.DOCUMENT_POSITION_FOLLOWING)
      && !!(roster.compareDocumentPosition(link) & Node.DOCUMENT_POSITION_FOLLOWING);
  });
  expect(order).toBe(true);
  await expect(main.getByRole("link", {name: "查看完整名单", exact: true})).toHaveCount(0);
  await expect(main.getByRole("link", {name: "去赞助", exact: true})).toHaveCount(1);
}

test.describe("首页", () => {
  test("首次运行展示首页统计和四个主入口，设置引导是独立次要入口", async ({page}) => {
    await installMockTauri(page, {onboarding: true, dashboardStats: {managedGameCount: 0, installedModCount: 0, totalModCount: 0}});
    await page.goto("/");

    await expect(sidebar(page).getByRole("button", {name: "首页", exact: true})).toHaveAttribute("aria-current", "page");
    await expect(home(page).getByRole("heading", {name: "欢迎来到 Agent Mod Manager", exact: true, level: 1})).toBeVisible();
    await expect(homeHeader(page).getByRole("heading", {name: "管理的游戏", exact: true, level: 2})).toBeVisible();
    await expect(homeHeader(page).getByRole("heading", {name: "已安装的 Mod / 全部 Mod", exact: true, level: 2})).toBeVisible();
    await expect(homeHeader(page)).toContainText("0");
    await expect(home(page).locator("header img")).toHaveCount(0);
    await expectHomeActions(page);

    const setup = home(page).getByRole("button", {name: "开始设置", exact: true});
    await expect(setup).toBeVisible();
    await setup.click();
    await expect(page.getByRole("heading", {name: "设置数据存储目录", exact: true})).toBeVisible();
  });

  test("未完成的首次设置以独立次要入口继续", async ({page}) => {
    await installMockTauri(page, {onboarding: true, onboardingStarted: true});
    await page.goto("/");

    await expectHomeActions(page);
    const resume = home(page).getByRole("button", {name: "继续设置", exact: true});
    await expect(resume).toBeVisible();
    await resume.click();
    await expect(page.getByRole("heading", {name: "设置数据存储目录", exact: true})).toBeVisible();
  });

  test("无游戏时显示统计和赞助名单，所有主入口均可导航", async ({page}) => {
    await installMockTauri(page, {
      emptyLibrary: true,
      dashboardSponsors: ["Fixture Sponsor", "Second Fixture"],
      dashboardStats: {managedGameCount: 0, installedModCount: 0, totalModCount: 0},
    });
    await page.goto("/");

    await expect(home(page).getByRole("heading", {name: "欢迎来到 Agent Mod Manager", exact: true, level: 1})).toBeVisible();
    await expect(homeHeader(page).getByRole("heading", {name: "管理的游戏", exact: true, level: 2})).toBeVisible();
    await expect(homeHeader(page).getByRole("heading", {name: "已安装的 Mod / 全部 Mod", exact: true, level: 2})).toBeVisible();
    await expect(homeHeader(page)).toContainText("0");
    await expectHomeActions(page);
    await expect(home(page).getByRole("button", {name: "开始设置", exact: true})).toHaveCount(0);
    await expectSponsorBlock(page, ["Fixture Sponsor", "Second Fixture"]);
    await verifyNavigation(page);
  });

  test("有游戏时显示统计和游戏快捷入口，所有主入口均可导航", async ({page}) => {
    await installMockTauri(page, {
      dashboardSponsors: ["Fixture Sponsor", "Second Fixture"],
      dashboardStats: {managedGameCount: 1, installedModCount: 2, totalModCount: 5},
    });
    await page.goto("/");

    await expect(home(page).getByRole("heading", {name: "欢迎来到 Agent Mod Manager", exact: true, level: 1})).toBeVisible();
    await expect(homeHeader(page).getByRole("heading", {name: "管理的游戏", exact: true, level: 2})).toBeVisible();
    await expect(homeHeader(page).getByText("1", {exact: true})).toBeVisible();
    await expect(homeHeader(page).getByRole("heading", {name: "已安装的 Mod / 全部 Mod", exact: true, level: 2})).toBeVisible();
    await expect(homeHeader(page).getByText("2 / 5", {exact: true})).toBeVisible();
    const game = homeHeader(page).getByRole("button", {name: "Minecraft Java Edition", exact: true});
    await expect(game).toBeVisible();
    await expect(page.locator("main").getByText(new RegExp(`当前版本\\s*v${version}`))).toBeVisible();
    await expectHomeActions(page);
    await expectSponsorBlock(page, ["Fixture Sponsor", "Second Fixture"]);

    await game.click();
    await expect(page.getByTestId("game-detail-page")).toBeVisible();
    await returnHome(page);
    await verifyNavigation(page);
  });
});
