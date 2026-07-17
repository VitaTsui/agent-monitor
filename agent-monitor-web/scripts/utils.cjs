const path = require("path");
const fs = require("fs");

/**
 * 读取模板文件并替换占位符
 * @param {string} templatePath - 模板文件路径
 * @param {Object} replacements - 替换映射对象
 * @returns {string} 替换后的内容
 */
function readAndReplaceTemplate(templatePath, replacements) {
  try {
    let content = fs.readFileSync(templatePath, "utf8");

    // 移除模板字符串的标记
    content = content
      .replace(/^`\s*/, "")
      .replace(/\s*`;\s*$/, "")
      .trim();

    // 执行替换
    Object.keys(replacements).forEach((key) => {
      const regex = new RegExp(key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "g");
      content = content.replace(regex, replacements[key]);
    });

    return content;
  } catch (error) {
    console.error(`读取模板文件失败: ${templatePath}`, error);
    throw error;
  }
}

/**
 * 确保目录存在
 * @param {string} dirPath - 目录路径
 */
function ensureDirectoryExists(dirPath) {
  if (!fs.existsSync(dirPath)) {
    fs.mkdirSync(dirPath, { recursive: true });
  }
}

/**
 * 写入文件
 * @param {string} filePath - 文件路径
 * @param {string} content - 文件内容
 */
function writeFile(filePath, content) {
  try {
    ensureDirectoryExists(path.dirname(filePath));
    const fileSave = require("file-save");
    fileSave(filePath).write(content, "utf-8").end("\n");
    console.log(`✓ 创建文件: ${filePath}`);
  } catch (error) {
    console.error(`✗ 创建文件失败: ${filePath}`, error);
    throw error;
  }
}

/**
 * 验证必需参数
 * @param {string} panelName - 面板名称
 */
function validatePanelName(panelName) {
  if (!panelName) {
    console.error("错误: 请提供面板名称");
    process.exit(1);
  }

  // 验证名称格式（只允许字母、数字、下划线和连字符）
  if (!/^[a-zA-Z0-9_-]+$/.test(panelName)) {
    console.error("错误: 面板名称只能包含字母、数字、下划线和连字符");
    process.exit(1);
  }
}

/**
 * 构建 API 路径
 * @param {string} parent - 父级路径
 * @param {string} apiName - API 名称
 * @returns {string} API 路径
 */
function buildApiPath(parent, apiName) {
  return parent ? `${parent}/${apiName}` : apiName;
}

/**
 * 构建权限前缀
 * @param {string} apiUrl - API URL
 * @returns {string} 权限前缀
 */
function buildPermiPrefix(apiUrl) {
  if (!apiUrl) return "";
  return apiUrl.split("/").filter(Boolean).join(":");
}

module.exports = {
  readAndReplaceTemplate,
  ensureDirectoryExists,
  writeFile,
  validatePanelName,
  buildApiPath,
  buildPermiPrefix,
};
