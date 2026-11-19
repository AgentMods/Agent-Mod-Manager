// Opt-in, isolated smoke against the real Fabric/Minecraft server bootstrap.
// No accounts, no third-party Mods, and no acceptance of Minecraft's EULA.
import { createHash } from 'node:crypto';
import { closeSync, mkdirSync, mkdtempSync, openSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';

const minecraft = '1.21.1';
const loader = '0.16.14';
const serverSha1 = '59353fb40c36d304f2035d51e7d6e6baa98dc05c';
const maven = 'https://maven.fabricmc.net/';
const dependencies = [
  [`net.fabricmc:fabric-loader:${loader}`, '9ed7b7f4197153ad97d71b4b9cd9148989e8f0c600934b035ce45f3ef003efc0'],
  [`net.fabricmc:intermediary:${minecraft}`, '6059157dfb4a536ec151697004f74f6004b7d346b289c2f89849e0e585aa36fa'],
  ['org.ow2.asm:asm:9.8', '876eab6a83daecad5ca67eb9fcabb063c97b5aeb8cf1fca7a989ecde17522051'],
  ['org.ow2.asm:asm-analysis:9.8', 'e640732fbcd3c6271925a504f125e38384688f4dfbbf92c8622dfcee0d09edb9'],
  ['org.ow2.asm:asm-commons:9.8', '3301a1c1cb4c59fcc5292648dac1d7c5aed4c0f067dfbe88873b8cdfe77404f4'],
  ['org.ow2.asm:asm-tree:9.8', '14b7880cb7c85eed101e2710432fc3ffb83275532a6a894dc4c4095d49ad59f1'],
  ['org.ow2.asm:asm-util:9.8', '8ba0460ecb28fd0e2980e5f3ef3433a513a457bc077f81a53bdc75b587a08d15'],
  ['net.fabricmc:sponge-mixin:0.15.5+mixin.0.8.7', '01dd2b778ed5283bce6b6b07d9690d86d956b17a7103efafad47073db1599584'],
];

const root = mkdtempSync(join(tmpdir(), 'amm-fabric-smoke-'));
console.log(`Fabric smoke directory: ${root}`);
for (const name of ['deps', 'home', 'tmp']) mkdirSync(join(root, name));

function jdk(command) {
  const executable = process.platform === 'win32' ? `${command}.exe` : command;
  return process.env.JAVA_HOME ? join(process.env.JAVA_HOME, 'bin', executable) : executable;
}

// Do not inherit Java options, launcher credentials or the user's Java home.
const jdkEnv = Object.fromEntries(['PATH', 'Path', 'SystemRoot', 'WINDIR'].flatMap(
  key => process.env[key] ? [[key, process.env[key]]] : [],
));
Object.assign(jdkEnv, { HOME: join(root, 'home'), USERPROFILE: join(root, 'home'), TMPDIR: join(root, 'tmp'), TEMP: join(root, 'tmp'), TMP: join(root, 'tmp'), LANG: 'C' });

function lockedDownload(url, filename, algorithm, expected, expectedSize) {
  const path = join(root, 'deps', filename);
  execFileSync('curl', ['-q', '--fail', '--silent', '--show-error', '--proto', '=https', '--max-time', '90', '--max-filesize', '60000000', '--output', path, url]);
  const bytes = readFileSync(path);
  if ((expectedSize && bytes.length !== expectedSize) || createHash(algorithm).update(bytes).digest('hex') !== expected) {
    throw new Error(`Downloaded ${filename} does not match the pinned artifact`);
  }
  return path;
}

const version = execFileSync(jdk('javac'), ['-version'], { env: jdkEnv, encoding: 'utf8' });
if (!/^javac 21(?:\.|\s)/.test(version)) throw new Error('Use a JDK 21 (set JAVA_HOME if necessary)');
const server = lockedDownload(`https://piston-data.mojang.com/v1/objects/${serverSha1}/server.jar`, 'server.jar', 'sha1', serverSha1, 51_627_615);
const classpath = dependencies.map(([coordinate, hash]) => {
  const [group, artifact, version] = coordinate.split(':');
  const filename = `${artifact}-${version}.jar`;
  return lockedDownload(`${maven}${group.replaceAll('.', '/')}/${artifact}/${version}/${filename}`, filename, 'sha256', hash);
}).join(delimiter);

const source = join(root, 'FixtureMod.java');
writeFileSync(source, `
package ammsmoke;
import net.fabricmc.api.ModInitializer;
public final class FixtureMod implements ModInitializer {
    public void onInitialize() {
        System.out.println("AMM_FABRIC_FIXTURE_ENTRYPOINT");
    }
}
`);

function buildInstance(mode, depends, olderDependency = false) {
  const classes = join(root, `classes-${mode}`);
  mkdirSync(classes);
  execFileSync(jdk('javac'), ['-proc:none', '-cp', classpath, '-d', classes, source], { cwd: root, env: jdkEnv });
  writeFileSync(join(classes, 'fabric.mod.json'), JSON.stringify({
    schemaVersion: 1,
    id: 'amm_smoke_probe',
    version: '1.0.0',
    name: 'AMM Smoke Probe',
    environment: '*',
    entrypoints: { main: ['ammsmoke.FixtureMod'] },
    depends,
  }));
  const instance = join(root, mode);
  mkdirSync(instance);
  mkdirSync(join(instance, 'mods'));
  writeFileSync(join(instance, 'eula.txt'), 'eula=false\n', { flag: 'wx' });
  writeFileSync(join(instance, 'server.properties'), 'server-ip=127.0.0.1\nserver-port=0\n', { flag: 'wx' });
  execFileSync(jdk('jar'), ['--create', '--file', join(instance, 'mods', 'amm-smoke.jar'), '-C', classes, '.'], { cwd: root, env: jdkEnv });
  if (olderDependency) {
    const metadata = join(root, `older-dependency-${mode}`);
    mkdirSync(metadata);
    writeFileSync(join(metadata, 'fabric.mod.json'), JSON.stringify({
      schemaVersion: 1,
      id: 'amm_smoke_required',
      name: 'AMM Smoke Dependency',
      version: '1.0.0',
    }));
    execFileSync(jdk('jar'), ['--create', '--file', join(instance, 'mods', 'amm-dependency.jar'), '-C', metadata, '.'], { cwd: root, env: jdkEnv });
  }
  return instance;
}

function launch(instance, language = 'en', country = 'US') {
  const consoleLog = join(instance, 'fabric-console.log');
  const fd = openSync(consoleLog, 'wx');
  let result;
  try {
    result = spawnSync(jdk('java'), [
      '-Xmx1024m', '-Djava.awt.headless=true', `-Duser.language=${language}`, `-Duser.country=${country}`,
      `-Duser.home=${jdkEnv.HOME}`, `-Djava.io.tmpdir=${jdkEnv.TMPDIR}`, `-Dfabric.gameJarPath=${server}`,
      '-cp', classpath, 'net.fabricmc.loader.impl.launch.knot.KnotServer', 'nogui',
    ], { cwd: instance, stdio: ['ignore', fd, fd], env: jdkEnv, timeout: 120_000, killSignal: 'SIGKILL' });
  } finally {
    closeSync(fd);
  }
  if (result.error || result.signal) throw new Error(`Fabric bootstrap did not finish (${result.error || result.signal})`);
  const latestLog = join(instance, 'logs', 'latest.log');
  const log = readFileSync(latestLog, 'utf8');
  if (!log.includes(`Loading Minecraft ${minecraft} with Fabric Loader ${loader}`)) {
    throw new Error(`Fabric did not reach the pinned loader bootstrap; inspect ${consoleLog}`);
  }
  if (!/^eula=false$/m.test(readFileSync(join(instance, 'eula.txt'), 'utf8'))) throw new Error('EULA guard no longer false');
  return { log, status: result.status, latestLog };
}

const requiresDependency = { amm_smoke_required: '>=2.0.0' };
for (const [mode, olderDependency, expected, language, country] of [
  ['missing-dependency', false, "Mod 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 requires version 2.0.0 or later of amm_smoke_required, which is missing!"],
  ['wrong-version', true, "Mod 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 requires version 2.0.0 or later of mod 'AMM Smoke Dependency' (amm_smoke_required), but only the wrong version is present: 1.0.0!"],
  ['zh-missing-dependency', false, "模组 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 需要 amm_smoke_required 的 2.0.0 及以上版本，但没有安装它！", 'zh', 'CN'],
  ['zh-wrong-version', true, "模组 'AMM Smoke Probe' (amm_smoke_probe) 1.0.0 需要 模组 'AMM Smoke Dependency' (amm_smoke_required) 的 2.0.0 及以上版本，但已经安装了的版本 1.0.0 不对！", 'zh', 'CN'],
]) {
  const failed = launch(buildInstance(mode, requiresDependency, olderDependency), language, country);
  const lines = failed.log.split(/\r?\n/);
  const heading = language === 'zh' ? '更多信息：' : 'More details:';
  const detail = lines.findIndex((line, i) => line === `\t - ${expected}` && lines[i - 1] === heading);
  if (failed.status !== 1 || !failed.log.includes('Mod resolution failed') || detail === -1 || failed.log.includes('AMM_FABRIC_FIXTURE_ENTRYPOINT')) {
    throw new Error(`Expected ${mode} before the fixture entrypoint; inspect ${failed.latestLog}`);
  }
  console.log(`Real Fabric ${mode} failure captured: ${failed.latestLog}`);
  if (language === 'zh') console.log(`${failed.latestLog}:${detail + 1}:${lines[detail]}`);
}

const base = launch(buildInstance('base', { minecraft, fabricloader: loader }));
const [selected, entered, stopped] = [
  '\n\t- amm_smoke_probe 1.0.0',
  'AMM_FABRIC_FIXTURE_ENTRYPOINT',
  'You need to agree to the EULA',
].map(marker => base.log.indexOf(marker));
if (base.status !== 0 || selected === -1 || entered <= selected || stopped <= entered) {
  throw new Error(`Expected selection, entrypoint, and stop at the unsigned EULA in that order; inspect ${base.latestLog}`);
}
console.log(`Real Fabric selection and fixture entrypoint before EULA stop: ${base.latestLog}`);
