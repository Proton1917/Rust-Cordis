import {dirname, resolve} from "node:path";
import {fileURLToPath} from "node:url";
import ts from "typescript";

/** DOC-15 §10：SDK 与网页宿主共用项目配置及 TypeScript 编译诊断。 */
export function compile(directory) {
    const path = resolve(directory, "tsconfig.json");
    const format = {getCanonicalFileName: path => path, getCurrentDirectory: () => directory, getNewLine: () => "\n"};
    const loaded = ts.readConfigFile(path, ts.sys.readFile);
    if (loaded.error) throw new Error(ts.formatDiagnosticsWithColorAndContext([loaded.error], format));
    const config = ts.parseJsonConfigFileContent(loaded.config, ts.sys, dirname(path));
    const program = ts.createProgram({rootNames: config.fileNames, options: config.options,
        projectReferences: config.projectReferences, configFileParsingDiagnostics: config.errors});
    const diagnostics = ts.getPreEmitDiagnostics(program);
    if (diagnostics.length) throw new Error(ts.formatDiagnosticsWithColorAndContext(diagnostics, format));
    const emitted = program.emit();
    if (emitted.emitSkipped && !config.options.noEmit) {
        throw new Error(ts.formatDiagnosticsWithColorAndContext(emitted.diagnostics, format));
    }
}

const current = fileURLToPath(import.meta.url);
if (process.argv[1] && resolve(process.argv[1]) === current) compile(dirname(current));
