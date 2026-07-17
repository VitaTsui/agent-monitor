const path = require("path");
const uppercamelcase = require("uppercamelcase");
const {
  readAndReplaceTemplate,
  writeFile,
  validatePanelName,
  buildApiPath,
} = require("./utils.cjs");

/**
 * 创建表单面板
 * 用法: node createFormPanel.cjs --panel <panelName> [--parent <parent>] [--api <apiName>] [--apiUrl <apiUrl>] [--apiCrt] [--unForm]
 * 示例: node createFormPanel.cjs --panel userForm --parent dataMonitoring --api user --apiUrl /api/user --apiCrt
 */
function main() {
  try {
    // 解析命令行参数
    let panelIdx = process.argv.indexOf("--panel");
    panelIdx = panelIdx === -1 ? 1 : panelIdx;
    const panelName = process.argv[panelIdx + 1];

    validatePanelName(panelName);

    const PanelName = uppercamelcase(panelName);

    const parentIdx = process.argv.indexOf("--parent");
    const parent = (parentIdx !== -1 && process.argv[parentIdx + 1]) || "";

    const apiIdx = process.argv.indexOf("--api");
    const apiName =
      (apiIdx !== -1 && process.argv[apiIdx + 1]) || PanelName.toLowerCase();

    const apiUrlIdx = process.argv.indexOf("--apiUrl");
    const apiUrl = (apiUrlIdx !== -1 && process.argv[apiUrlIdx + 1]) || "";

    const apiPath = path.resolve(__dirname, "../src/services/apis", parent);
    const apiPathValue = buildApiPath(parent, apiName);

    const apiCrt = process.argv.includes("--apiCrt");
    const unForm = process.argv.includes("--unForm");

    // 确定面板路径
    let panelPath = path.resolve(
      __dirname,
      "../src/pages",
      parent,
      parent ? "" : parent.toLowerCase(),
      `${PanelName}Form`
    );

    if (unForm) {
      panelPath = path.resolve(__dirname, "../src/pages", parent, PanelName);
    }

    console.log(`\n开始创建表单面板: ${PanelName}`);
    console.log(`面板路径: ${panelPath}`);
    console.log(`API 路径: ${apiPathValue}`);
    console.log(`API URL: ${apiUrl || "无"}`);
    console.log(`创建 API 文件: ${apiCrt ? "是" : "否"}`);
    console.log(`独立表单: ${unForm ? "是" : "否"}\n`);

    // 读取模板
    const panelTemplate = path.resolve(__dirname, "./FormPanel/panel.js");
    const storeTemplate = path.resolve(__dirname, "./FormPanel/store.js");
    const apiTemplate = path.resolve(__dirname, "./FormPanel/api.js");

    // 处理面板模板（根据 unForm 标志决定替换模式）
    const panelReplacements = unForm
      ? { "<NAME>Form": PanelName, "<NAME>": PanelName }
      : { "<NAME>": PanelName };

    const Panel = readAndReplaceTemplate(panelTemplate, panelReplacements);

    const Store = readAndReplaceTemplate(storeTemplate, {
      "<NAME>": PanelName,
      "<API_PATH>": apiPathValue,
    });

    const Api = readAndReplaceTemplate(apiTemplate, {
      "<NAME>": PanelName,
      "<API_PAGE_PATH>": apiUrl ? `${apiUrl}/page` : "",
      "<API_INFO_PATH>": apiUrl ? `${apiUrl}/info/` : "",
      "<API_ADD_PATH>": apiUrl ? `${apiUrl}/add` : "",
      "<API_UPD_PATH>": apiUrl ? `${apiUrl}/upd` : "",
      "<API_DEL_PATH>": apiUrl ? `${apiUrl}/del` : "",
    });

    // 创建文件
    const files = [
      {
        fileName: "index.tsx",
        content: Panel,
        path: panelPath,
      },
      {
        fileName: unForm ? `${PanelName}Store.ts` : `${PanelName}FormStore.ts`,
        content: Store,
        path: panelPath,
      },
      {
        fileName: "index.module.scss",
        content: unForm ? `.${PanelName} {\n}` : `.${PanelName}Form {\n}`,
        path: panelPath,
      },
    ];

    files.forEach((file) => {
      writeFile(path.join(file.path, file.fileName), file.content);
    });

    // 可选：创建 API 文件
    if (apiCrt) {
      writeFile(path.join(apiPath, `${apiName}.ts`), Api);
    }

    console.log(`\n✓ 成功创建表单面板: ${PanelName}\n`);
  } catch (error) {
    console.error("\n✗ 创建表单面板失败:", error.message);
    process.exit(1);
  }
}

main();
