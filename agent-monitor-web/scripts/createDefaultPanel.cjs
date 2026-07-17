const path = require("path");
const uppercamelcase = require("uppercamelcase");
const {
  readAndReplaceTemplate,
  writeFile,
  validatePanelName,
  buildApiPath,
} = require("./utils.cjs");

/**
 * 创建默认面板
 * 用法: node createDefaultPanel.cjs <panelName> [parent] [apiName]
 * 示例: node createDefaultPanel.cjs dashboard dataMonitoring dashboard
 */
function main() {
  try {
    const panelName = process.argv[2];
    validatePanelName(panelName);

    const PanelName = uppercamelcase(panelName);
    const parent = process.argv[3] || "";
    const apiNameArg = process.argv[4] || "";
    const apiName = apiNameArg || PanelName.toLowerCase();

    const panelPath = path.resolve(
      __dirname,
      "../src/pages",
      parent,
      PanelName
    );
    const apiPath = path.resolve(__dirname, "../src/services/apis", parent);
    const apiPathValue = buildApiPath(parent, apiName);

    console.log(`\n开始创建默认面板: ${PanelName}`);
    console.log(`面板路径: ${panelPath}`);
    console.log(`API 路径: ${apiPathValue}\n`);

    // 读取并处理模板
    const panelTemplate = path.resolve(__dirname, "./DefaultPanel/panel.js");
    const storeTemplate = path.resolve(__dirname, "./DefaultPanel/store.js");
    const apiTemplate = path.resolve(__dirname, "./DefaultPanel/api.js");

    const Panel = readAndReplaceTemplate(panelTemplate, {
      "<NAME>": PanelName,
    });

    const Store = readAndReplaceTemplate(storeTemplate, {
      "<NAME>": PanelName,
    });

    const Api = readAndReplaceTemplate(apiTemplate, {
      "<NAME>": PanelName,
    });

    // 创建文件
    const files = [
      {
        fileName: "index.tsx",
        content: Panel,
        path: panelPath,
      },
      {
        fileName: `${PanelName}Store.ts`,
        content: Store,
        path: panelPath,
      },
      {
        fileName: "index.module.scss",
        content: `.${PanelName} {\n}\n\n.${PanelName}Content {\n}`,
        path: panelPath,
      },
      {
        fileName: `${apiName}.ts`,
        content: Api,
        path: apiPath,
      },
    ];

    files.forEach((file) => {
      writeFile(path.join(file.path, file.fileName), file.content);
    });

    console.log(`\n✓ 成功创建默认面板: ${PanelName}\n`);
  } catch (error) {
    console.error("\n✗ 创建默认面板失败:", error.message);
    process.exit(1);
  }
}

main();

