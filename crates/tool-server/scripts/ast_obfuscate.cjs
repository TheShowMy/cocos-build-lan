const fs = require('node:fs');
const path = require('node:path');

function parseArgs(argv) {
  const result = {
    deadCode: false,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const current = argv[index];
    if (current === '--dead-code') {
      result.deadCode = true;
      continue;
    }

    if (!current.startsWith('--')) {
      throw new Error(`无法识别的参数: ${current}`);
    }

    const key = current.slice(2);
    const normalizedKey = key.replace(/-([a-z])/g, (_, char) => char.toUpperCase());
    const value = argv[index + 1];
    if (value === undefined) {
      throw new Error(`缺少参数值: ${current}`);
    }
    result[normalizedKey] = value;
    index += 1;
  }

  return result;
}

function createRng(seedValue) {
  let state = BigInt.asUintN(64, BigInt(seedValue));
  return () => {
    state = BigInt.asUintN(64, state * 6364136223846793005n + 1442695040888963407n);
    return Number(state & 0xffffffffn) / 0x100000000;
  };
}

function collectIdentifiers(ts, node, set) {
  if (ts.isIdentifier(node)) {
    set.add(node.text);
  }
  ts.forEachChild(node, (child) => collectIdentifiers(ts, child, set));
}

function isFunctionLikeNode(ts, node) {
  return ts.isFunctionDeclaration(node)
    || ts.isFunctionExpression(node)
    || ts.isArrowFunction(node)
    || ts.isMethodDeclaration(node)
    || ts.isGetAccessorDeclaration(node)
    || ts.isSetAccessorDeclaration(node)
    || ts.isConstructorDeclaration(node);
}

function isIdentifierBinding(ts, node) {
  return ts.isIdentifier(node);
}

function isRenamableIdentifier(text, protectedNames) {
  if (!text) {
    return false;
  }
  if (protectedNames.has(text)) {
    return false;
  }
  if (text === 'arguments' || text === 'eval' || text === 'undefined') {
    return false;
  }
  return true;
}

function collectFunctionScopedVarNames(ts, body, protectedNames) {
  const names = new Set();
  const visit = (node) => {
    if (isFunctionLikeNode(ts, node)) {
      return;
    }

    if (ts.isVariableDeclaration(node) && isIdentifierBinding(ts, node.name)) {
      const declarationList = node.parent;
      if (ts.isVariableDeclarationList(declarationList)
        && (declarationList.flags & ts.NodeFlags.BlockScoped) === 0
        && isRenamableIdentifier(node.name.text, protectedNames)) {
        names.add(node.name.text);
      }
    }

    ts.forEachChild(node, visit);
  };

  ts.forEachChild(body, visit);
  return names;
}

function collectBlockScopedNames(ts, block, protectedNames) {
  const names = new Set();

  for (const statement of block.statements) {
    if (ts.isVariableStatement(statement)) {
      const declarationList = statement.declarationList;
      if ((declarationList.flags & ts.NodeFlags.BlockScoped) !== 0) {
        for (const declaration of declarationList.declarations) {
          if (isIdentifierBinding(ts, declaration.name)
            && isRenamableIdentifier(declaration.name.text, protectedNames)) {
            names.add(declaration.name.text);
          }
        }
      }
    } else if (ts.isForStatement(statement) && statement.initializer
      && ts.isVariableDeclarationList(statement.initializer)
      && (statement.initializer.flags & ts.NodeFlags.BlockScoped) !== 0) {
      for (const declaration of statement.initializer.declarations) {
        if (isIdentifierBinding(ts, declaration.name)
          && isRenamableIdentifier(declaration.name.text, protectedNames)) {
          names.add(declaration.name.text);
        }
      }
    } else if ((ts.isForInStatement(statement) || ts.isForOfStatement(statement))
      && ts.isVariableDeclarationList(statement.initializer)
      && (statement.initializer.flags & ts.NodeFlags.BlockScoped) !== 0) {
      for (const declaration of statement.initializer.declarations) {
        if (isIdentifierBinding(ts, declaration.name)
          && isRenamableIdentifier(declaration.name.text, protectedNames)) {
          names.add(declaration.name.text);
        }
      }
    }
  }

  return names;
}

function shouldRenameIdentifier(ts, node) {
  const parent = node.parent;
  if (!parent) {
    return true;
  }

  if (ts.isPropertyAccessExpression(parent) && parent.name === node) {
    return false;
  }
  if (ts.isPropertyAssignment(parent) && parent.name === node) {
    return false;
  }
  if (ts.isShorthandPropertyAssignment(parent) && parent.name === node) {
    return false;
  }
  if (ts.isMethodDeclaration(parent) && parent.name === node) {
    return false;
  }
  if (ts.isGetAccessorDeclaration(parent) && parent.name === node) {
    return false;
  }
  if (ts.isSetAccessorDeclaration(parent) && parent.name === node) {
    return false;
  }
  if (ts.isPropertyDeclaration(parent) && parent.name === node) {
    return false;
  }
  if (ts.isPropertySignature(parent) && parent.name === node) {
    return false;
  }
  if (ts.isBindingElement(parent) && parent.propertyName === node) {
    return false;
  }
  if (ts.isImportSpecifier(parent) || ts.isExportSpecifier(parent)) {
    return false;
  }
  if (ts.isLabeledStatement(parent) && parent.label === node) {
    return false;
  }
  if (ts.isBreakStatement(parent) && parent.label === node) {
    return false;
  }
  if (ts.isContinueStatement(parent) && parent.label === node) {
    return false;
  }
  if (ts.isClassDeclaration(parent) && parent.name === node) {
    return false;
  }
  if (ts.isFunctionDeclaration(parent) && parent.name === node) {
    return false;
  }
  return true;
}

function isDirectiveLiteral(ts, node) {
  const parent = node.parent;
  return ts.isExpressionStatement(parent)
    && parent.expression === node
    && (ts.isBlock(parent.parent) || ts.isSourceFile(parent.parent))
    && parent.parent.statements.indexOf(parent) >= 0
    && parent.parent.statements
      .slice(0, parent.parent.statements.indexOf(parent))
      .every((statement) => ts.isExpressionStatement(statement) && ts.isStringLiteral(statement.expression));
}

function isSafeLiteralRewriteContext(ts, node) {
  const parent = node.parent;
  if (!parent) {
    return false;
  }

  if (ts.isVariableDeclaration(parent) && parent.initializer === node) {
    return true;
  }
  if (ts.isBinaryExpression(parent) && (parent.left === node || parent.right === node)) {
    return true;
  }
  if (ts.isReturnStatement(parent) && parent.expression === node) {
    return true;
  }
  if (ts.isCallExpression(parent) && parent.arguments.includes(node)) {
    return true;
  }
  if (ts.isNewExpression(parent) && parent.arguments?.includes(node)) {
    return true;
  }
  if (ts.isArrayLiteralExpression(parent) && parent.elements.includes(node)) {
    return true;
  }
  if (ts.isParenthesizedExpression(parent) && parent.expression === node) {
    return true;
  }
  if (ts.isConditionalExpression(parent)) {
    return parent.condition === node || parent.whenTrue === node || parent.whenFalse === node;
  }
  if (ts.isExpressionStatement(parent) && parent.expression === node) {
    return !isDirectiveLiteral(ts, node);
  }
  if (ts.isIfStatement(parent) && parent.expression === node) {
    return true;
  }
  if (ts.isWhileStatement(parent) && parent.expression === node) {
    return true;
  }
  if (ts.isDoStatement(parent) && parent.expression === node) {
    return true;
  }

  return false;
}

function randomChar(rng, alphabet) {
  return alphabet[Math.floor(rng() * alphabet.length)];
}

function createRandomIdentifier(rng) {
  const headAlphabet = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ$_';
  const bodyAlphabet = `${headAlphabet}0123456789`;
  const targetLength = 6 + Math.floor(rng() * 7);

  let result = randomChar(rng, headAlphabet);
  for (let index = 1; index < targetLength; index += 1) {
    result += randomChar(rng, bodyAlphabet);
  }
  return result;
}

function createNameGenerator(rng, forbiddenNames) {
  return () => {
    while (true) {
      const name = createRandomIdentifier(rng);
      if (!forbiddenNames.has(name)) {
        forbiddenNames.add(name);
        return name;
      }
    }
  };
}

class Scope {
  constructor(parent = null) {
    this.parent = parent;
    this.mappings = new Map();
  }

  resolve(name) {
    if (this.mappings.has(name)) {
      return this.mappings.get(name);
    }
    return this.parent ? this.parent.resolve(name) : undefined;
  }
}

function collectCandidateFunctions(ts, root) {
  const candidates = [];

  const visit = (node) => {
    if (isFunctionLikeNode(ts, node) && node.body && ts.isBlock(node.body) && node.body.statements.length > 0) {
      candidates.push(node);
    }
    ts.forEachChild(node, visit);
  };

  visit(root);
  candidates.sort((left, right) => left.getStart(root) - right.getStart(root));
  return candidates;
}

function assignDeadCodePlan(candidateFunctions, targetCount) {
  const plan = new WeakMap();
  if (targetCount <= 0 || candidateFunctions.length === 0) {
    return {
      plan,
      actualCount: 0,
      shortageReason: targetCount > 0 && candidateFunctions.length === 0 ? '未找到可注入的函数体' : null,
    };
  }

  const maxPerFunction = 4;
  const counts = new Array(candidateFunctions.length).fill(0);
  let assigned = 0;

  while (assigned < targetCount) {
    const remaining = targetCount - assigned;
    const passSize = Math.min(candidateFunctions.length, remaining);
    const usedThisPass = new Set();
    let progressed = false;

    for (let slot = 0; slot < passSize; slot += 1) {
      const preferredIndex = Math.floor(((slot + 0.5) * candidateFunctions.length) / passSize);
      let chosenIndex = -1;

      for (let offset = 0; offset < candidateFunctions.length; offset += 1) {
        const candidatesToTry = [];
        if (offset === 0) {
          candidatesToTry.push(preferredIndex);
        } else {
          candidatesToTry.push(preferredIndex - offset, preferredIndex + offset);
        }

        for (const index of candidatesToTry) {
          if (index < 0 || index >= candidateFunctions.length) {
            continue;
          }
          if (usedThisPass.has(index) || counts[index] >= maxPerFunction) {
            continue;
          }
          chosenIndex = index;
          break;
        }

        if (chosenIndex !== -1) {
          break;
        }
      }

      if (chosenIndex === -1) {
        continue;
      }

      counts[chosenIndex] += 1;
      usedThisPass.add(chosenIndex);
      assigned += 1;
      progressed = true;
      if (assigned >= targetCount) {
        break;
      }
    }

    if (!progressed) {
      break;
    }
  }

  let actualCount = 0;
  counts.forEach((count, index) => {
    if (count > 0) {
      plan.set(candidateFunctions[index], count);
      actualCount += count;
    }
  });

  return {
    plan,
    actualCount,
    shortageReason: actualCount < targetCount
      ? `安全候选点不足，目标 ${targetCount}，实际 ${actualCount}`
      : null,
  };
}

function chooseInsertionIndexes(statementCount, injectCount, rng) {
  const boundaryCount = statementCount + 1;
  if (injectCount <= 0) {
    return [];
  }

  const result = [];
  for (let index = 0; index < injectCount; index += 1) {
    let position = Math.floor(((index + 1) * boundaryCount) / (injectCount + 1));
    if (boundaryCount > 4) {
      const jitter = rng() < 0.5 ? -1 : 1;
      position = Math.max(0, Math.min(statementCount, position + jitter));
    }
    result.push(position);
  }

  result.sort((left, right) => left - right);
  return result;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const requiredKeys = ['input', 'output', 'whitelist'];
  for (const key of requiredKeys) {
    if (!args[key]) {
      throw new Error(`缺少必填参数 --${key}`);
    }
  }

  const inputPath = path.resolve(args.input);
  const outputPath = path.resolve(args.output);
  const whitelistPath = path.resolve(args.whitelist);
  const ts = args.typescript
    ? require(path.resolve(args.typescript))
    : require('typescript');
  const code = fs.readFileSync(inputPath, 'utf8');
  const whitelist = JSON.parse(fs.readFileSync(whitelistPath, 'utf8'));
  const sourceFile = ts.createSourceFile(inputPath, code, ts.ScriptTarget.ES2022, true, ts.ScriptKind.JS);
  if (sourceFile.parseDiagnostics.length > 0) {
    throw new Error(`原始 game.js AST 解析失败: ${sourceFile.parseDiagnostics[0].messageText}`);
  }

  const protectedNames = new Set([
    ...(whitelist.engine_keywords || []),
    ...(whitelist.exclude_keywords || []),
    ...(whitelist.js_keywords || []),
    ...(whitelist.merged_keywords || []),
    'System',
    'exports',
    'module',
    'require',
    'window',
    'globalThis',
    'cc',
    '__decorate',
    '__esDecorate',
    '__runInitializers',
    '__setFunctionName',
  ]);
  const forbiddenNames = new Set(protectedNames);
  collectIdentifiers(ts, sourceFile, forbiddenNames);

  const rng = createRng(args.seed ? BigInt(args.seed) : BigInt(Date.now()));
  const nextName = createNameGenerator(rng, forbiddenNames);
  const deadCodeTargetCount = args.deadCode
    ? Math.max(1, Number.parseInt(args.deadCodeCount ?? '200', 10) || 200)
    : 0;
  const candidateFunctions = collectCandidateFunctions(ts, sourceFile);
  const deadCodePlanResult = assignDeadCodePlan(candidateFunctions, deadCodeTargetCount);
  const stats = {
    renamedBindingCount: 0,
    rewrittenExpressionCount: 0,
    rewrittenLiteralCount: 0,
    deadCodeTargetCount,
    deadCodeActualCount: deadCodePlanResult.actualCount,
    deadCodeBlockCount: deadCodePlanResult.actualCount,
    candidateFunctionCount: candidateFunctions.length,
    deadCodeShortageReason: deadCodePlanResult.shortageReason,
  };

  function registerMapping(scope, originalName) {
    if (!scope.mappings.has(originalName)) {
      scope.mappings.set(originalName, nextName());
      stats.renamedBindingCount += 1;
    }
  }

  function maybeRewriteLiteral(node) {
    if (node.kind === ts.SyntaxKind.TrueKeyword && isSafeLiteralRewriteContext(ts, node) && rng() < 0.06) {
      stats.rewrittenExpressionCount += 1;
      return ts.factory.createPrefixUnaryExpression(
        ts.SyntaxKind.ExclamationToken,
        ts.factory.createNumericLiteral(0),
      );
    }

    if (node.kind === ts.SyntaxKind.FalseKeyword && isSafeLiteralRewriteContext(ts, node) && rng() < 0.06) {
      stats.rewrittenExpressionCount += 1;
      return ts.factory.createPrefixUnaryExpression(
        ts.SyntaxKind.ExclamationToken,
        ts.factory.createNumericLiteral(1),
      );
    }

    if (ts.isStringLiteral(node)
      && isSafeLiteralRewriteContext(ts, node)
      && node.text.length >= 8
      && rng() < 0.04) {
      const splitIndex = Math.max(1, Math.floor(node.text.length / 2));
      stats.rewrittenLiteralCount += 1;
      return ts.factory.createBinaryExpression(
        ts.factory.createStringLiteral(node.text.slice(0, splitIndex)),
        ts.SyntaxKind.PlusToken,
        ts.factory.createStringLiteral(node.text.slice(splitIndex)),
      );
    }

    return node;
  }

  function createRandomStringLiteral(prefix) {
    const suffixLength = 8 + Math.floor(rng() * 12);
    const alphabet = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
    let suffix = '';
    for (let index = 0; index < suffixLength; index += 1) {
      suffix += randomChar(rng, alphabet);
    }
    return `${prefix}_${suffix}`;
  }

  function createNumericExpression() {
    const left = 1000 + Math.floor(rng() * 900000);
    const right = 10 + Math.floor(rng() * 5000);
    const operator = rng() < 0.5 ? ts.SyntaxKind.PlusToken : ts.SyntaxKind.MinusToken;
    return ts.factory.createBinaryExpression(
      ts.factory.createNumericLiteral(left),
      operator,
      ts.factory.createNumericLiteral(right),
    );
  }

  function createDeadCodeStatement() {
    const deadName = nextName();
    const deadName2 = nextName();
    const templateIndex = Math.floor(rng() * 4);

    if (templateIndex === 0) {
      return ts.factory.createIfStatement(
        ts.factory.createFalse(),
        ts.factory.createBlock([
          ts.factory.createVariableStatement(
            undefined,
            ts.factory.createVariableDeclarationList(
              [
                ts.factory.createVariableDeclaration(
                  ts.factory.createIdentifier(deadName),
                  undefined,
                  undefined,
                  ts.factory.createBinaryExpression(
                    ts.factory.createStringLiteral(createRandomStringLiteral('dead')),
                    ts.SyntaxKind.PlusToken,
                    ts.factory.createStringLiteral(createRandomStringLiteral('noise')),
                  ),
                ),
              ],
              ts.NodeFlags.Const,
            ),
          ),
        ], true),
        undefined,
      );
    }

    if (templateIndex === 1) {
      return ts.factory.createIfStatement(
        ts.factory.createBinaryExpression(
          ts.factory.createNumericLiteral(1),
          ts.SyntaxKind.EqualsEqualsEqualsToken,
          ts.factory.createNumericLiteral(2),
        ),
        ts.factory.createBlock([
          ts.factory.createVariableStatement(
            undefined,
            ts.factory.createVariableDeclarationList(
              [
                ts.factory.createVariableDeclaration(
                  ts.factory.createIdentifier(deadName),
                  undefined,
                  undefined,
                  ts.factory.createArrayLiteralExpression([
                    createNumericExpression(),
                    createNumericExpression(),
                    createNumericExpression(),
                  ], false),
                ),
              ],
              ts.NodeFlags.Const,
            ),
          ),
        ], true),
        undefined,
      );
    }

    if (templateIndex === 2) {
      return ts.factory.createVariableStatement(
        undefined,
        ts.factory.createVariableDeclarationList(
          [
            ts.factory.createVariableDeclaration(
              ts.factory.createIdentifier(deadName),
              undefined,
              undefined,
              ts.factory.createObjectLiteralExpression([
                ts.factory.createPropertyAssignment(
                  ts.factory.createIdentifier(nextName()),
                  ts.factory.createStringLiteral(createRandomStringLiteral('trace')),
                ),
                ts.factory.createPropertyAssignment(
                  ts.factory.createIdentifier(nextName()),
                  createNumericExpression(),
                ),
                ts.factory.createPropertyAssignment(
                  ts.factory.createIdentifier(nextName()),
                  ts.factory.createFalse(),
                ),
              ], false),
            ),
          ],
          ts.NodeFlags.Const,
        ),
      );
    }

    return ts.factory.createIfStatement(
      ts.factory.createFalse(),
      ts.factory.createBlock([
        ts.factory.createVariableStatement(
          undefined,
          ts.factory.createVariableDeclarationList(
            [
              ts.factory.createVariableDeclaration(
                ts.factory.createIdentifier(deadName),
                undefined,
                undefined,
                createNumericExpression(),
              ),
              ts.factory.createVariableDeclaration(
                ts.factory.createIdentifier(deadName2),
                undefined,
                undefined,
                ts.factory.createBinaryExpression(
                  ts.factory.createStringLiteral(createRandomStringLiteral('mask')),
                  ts.SyntaxKind.PlusToken,
                  ts.factory.createStringLiteral(createRandomStringLiteral('value')),
                ),
              ),
            ],
            ts.NodeFlags.Const,
          ),
        ),
      ], true),
      undefined,
    );
  }

  function maybeInjectDeadCode(node) {
    if (!args.deadCode || !node.body || !ts.isBlock(node.body)) {
      return node;
    }
    const injectCount = deadCodePlanResult.plan.get(node) ?? 0;
    if (injectCount <= 0 || node.body.statements.length === 0) {
      return node;
    }

    const insertionIndexes = chooseInsertionIndexes(node.body.statements.length, injectCount, rng);
    const nextStatements = [];

    for (let boundary = 0; boundary <= node.body.statements.length; boundary += 1) {
      insertionIndexes.forEach((index) => {
        if (index === boundary) {
          nextStatements.push(createDeadCodeStatement());
        }
      });
      if (boundary < node.body.statements.length) {
        nextStatements.push(node.body.statements[boundary]);
      }
    }

    const nextBody = ts.factory.updateBlock(node.body, nextStatements);

    if (ts.isFunctionDeclaration(node)) {
      return ts.factory.updateFunctionDeclaration(
        node,
        node.modifiers,
        node.asteriskToken,
        node.name,
        node.typeParameters,
        node.parameters,
        node.type,
        nextBody,
      );
    }
    if (ts.isFunctionExpression(node)) {
      return ts.factory.updateFunctionExpression(
        node,
        node.modifiers,
        node.asteriskToken,
        node.name,
        node.typeParameters,
        node.parameters,
        node.type,
        nextBody,
      );
    }
    if (ts.isMethodDeclaration(node)) {
      return ts.factory.updateMethodDeclaration(
        node,
        node.modifiers,
        node.asteriskToken,
        node.name,
        node.questionToken,
        node.typeParameters,
        node.parameters,
        node.type,
        nextBody,
      );
    }
    if (ts.isConstructorDeclaration(node)) {
      return ts.factory.updateConstructorDeclaration(
        node,
        node.modifiers,
        node.parameters,
        nextBody,
      );
    }
    if (ts.isGetAccessorDeclaration(node)) {
      return ts.factory.updateGetAccessorDeclaration(
        node,
        node.modifiers,
        node.name,
        node.parameters,
        node.type,
        nextBody,
      );
    }
    if (ts.isSetAccessorDeclaration(node)) {
      return ts.factory.updateSetAccessorDeclaration(
        node,
        node.modifiers,
        node.name,
        node.parameters,
        nextBody,
      );
    }
    if (ts.isArrowFunction(node)) {
      return ts.factory.updateArrowFunction(
        node,
        node.modifiers,
        node.typeParameters,
        node.parameters,
        node.type,
        node.equalsGreaterThanToken,
        nextBody,
      );
    }
    return node;
  }

  const transformer = (context) => {
    const scopeStack = [new Scope(null)];

    const currentScope = () => scopeStack[scopeStack.length - 1];

    const visitor = (node) => {
      if (isFunctionLikeNode(ts, node)) {
        const fnScope = new Scope(currentScope());

        for (const parameter of node.parameters || []) {
          if (isIdentifierBinding(ts, parameter.name) && isRenamableIdentifier(parameter.name.text, protectedNames)) {
            registerMapping(fnScope, parameter.name.text);
          }
        }
        if (node.body && ts.isBlock(node.body)) {
          for (const name of collectFunctionScopedVarNames(ts, node.body, protectedNames)) {
            registerMapping(fnScope, name);
          }
        }

        scopeStack.push(fnScope);
        let visited = ts.visitEachChild(node, visitor, context);
        scopeStack.pop();
        visited = maybeInjectDeadCode(visited);
        return visited;
      }

      if (ts.isCatchClause(node)) {
        const catchScope = new Scope(currentScope());
        if (node.variableDeclaration
          && isIdentifierBinding(ts, node.variableDeclaration.name)
          && isRenamableIdentifier(node.variableDeclaration.name.text, protectedNames)) {
          registerMapping(catchScope, node.variableDeclaration.name.text);
        }

        scopeStack.push(catchScope);
        const visited = ts.visitEachChild(node, visitor, context);
        scopeStack.pop();
        return visited;
      }

      if (ts.isBlock(node)) {
        const blockScope = new Scope(currentScope());
        for (const name of collectBlockScopedNames(ts, node, protectedNames)) {
          registerMapping(blockScope, name);
        }
        scopeStack.push(blockScope);
        const visited = ts.visitEachChild(node, visitor, context);
        scopeStack.pop();
        return visited;
      }

      if (ts.isIdentifier(node) && shouldRenameIdentifier(ts, node)) {
        const renamed = currentScope().resolve(node.text);
        if (renamed) {
          return ts.factory.createIdentifier(renamed);
        }
      }

      const rewrittenNode = maybeRewriteLiteral(node);
      if (rewrittenNode !== node) {
        return rewrittenNode;
      }

      return ts.visitEachChild(node, visitor, context);
    };

    return (node) => ts.visitNode(node, visitor);
  };

  const result = ts.transform(sourceFile, [transformer]);
  const transformedFile = result.transformed[0];
  const printer = ts.createPrinter({
    newLine: ts.NewLineKind.LineFeed,
    removeComments: false,
  });
  const outputCode = printer.printFile(transformedFile);
  result.dispose();

  const reparsed = ts.createSourceFile(outputPath, outputCode, ts.ScriptTarget.ES2022, true, ts.ScriptKind.JS);
  if (reparsed.parseDiagnostics.length > 0) {
    throw new Error(`变换后的 game.js AST 复检失败: ${reparsed.parseDiagnostics[0].messageText}`);
  }

  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, outputCode, 'utf8');
  process.stdout.write(JSON.stringify(stats));
}

try {
  main();
} catch (error) {
  const message = error instanceof Error ? error.stack || error.message : String(error);
  process.stderr.write(`${message}\n`);
  process.exit(1);
}
