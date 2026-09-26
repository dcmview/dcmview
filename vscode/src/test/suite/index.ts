import Mocha from 'mocha';
import * as path from 'path';

export function run(): Promise<void> {
  const mocha = new Mocha({
    color: true,
    ui: 'tdd',
  });

  mocha.addFile(path.resolve(__dirname, 'extension.test.js'));
  mocha.addFile(path.resolve(__dirname, 'viewer.test.js'));

  return new Promise((resolve, reject) => {
    mocha.run((failures: number) => {
      if (failures > 0) {
        reject(new Error(`${failures} test(s) failed.`));
      } else {
        resolve();
      }
    });
  });
}
