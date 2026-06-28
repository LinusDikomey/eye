#!/usr/bin/env python3
import os
import subprocess
import time
from sys import stdout

cargo_test = ['cargo', 'test', '--all']
build = ['cargo', 'build']
eye_path = './target/debug/eye'
run_cmd = [eye_path, 'run']
tmp_file = 'tmp_test.eye'

GREEN = '\033[1;32m'
RED = '\033[1;31m'
CYAN = '\033[1;36m'
R = '\033[1;0m'

def main():
    cargo_test_ok = subprocess.run(cargo_test).returncode == 0
    
    start_time = time.time()

    total, errors = test_files()
    total2, errors2 = test_readme()
    total += total2
    errors += errors2

    elapsed = time.time() - start_time

    if errors > 0:
        print(f'{RED}{errors}/{total} tests failed!{R} in {elapsed:.2f}s')
    if not cargo_test_ok:
        print(f'{RED}Cargo Test(s) failed{R}')
        
    if errors > 0 or not cargo_test_ok:
        exit(1)
    else:
        print(f'{GREEN}All {total} tests passed!{R} in {elapsed:.2f}s')

def test_files():
    print(f'{CYAN}Compiling...{R}')
    if subprocess.run(build).returncode != 0:
        print(f'{RED}An error occurred while compiling{R}')
        exit(1)
    print(f'{CYAN}Running tests ...{R}')
    total = 1
    errors = 0
    if not check_std(): errors += 1
    tmp_total, tmp_errors = walk('eye')
    total += tmp_total
    errors += tmp_errors
    return total, errors
    
    


def test_readme():
    print(f'{CYAN}Running README tests ...{R}')
    file = open('README.md', encoding="utf8")
    # contents = file.read()
    current_source = ''
    in_code_block = False
    block_count = 0
    errors = 0
    for line in file:
        if line.find('```') != -1:
            if in_code_block:
                temp_code_file = open(tmp_file, 'w', encoding="utf8")
                temp_code_file.write(current_source)
                temp_code_file.close()
                
                print(f'Testing README block #{block_count} ...', end = '')
                out, stderr, exit_code = run(tmp_file)
                padding = ' ' * max(0, 28 - len(f'{block_count}'))
                if exit_code != 0:
                    print(f'{padding}{RED}[ERR]{R}')
                    print(f'{RED}Output{R}:\n{out}')
                    print(f'{RED}Stderr{R}:\n{stderr}')
                    errors += 1
                else:
                    print(f'{padding}{GREEN}[OK]{R}')
                current_source = ''
            else: block_count += 1
            in_code_block = not in_code_block
        elif in_code_block:
            current_source += line

    os.remove(tmp_file)
    return block_count, errors




def walk(walkdir):
    errors = 0
    total = 0
    with os.scandir(walkdir) as it:
        for entry in it:
            if entry.name.endswith('.eye') and entry.is_file():
                res = test(entry.path)
                if res is OK:
                    total += 1
                elif res is ERR:
                    total += 1
                    errors += 1
                else: assert res is SKIP
            elif entry.is_dir():
                if os.path.exists(entry.path + "/main.eye"):
                    res = test(entry.path)
                    if res is OK:
                        total += 1
                    elif res is ERR:
                        total += 1
                        errors += 1
                    else: assert res is SKIP
                else:
                    total2, errors2 = walk(entry.path)
                    total += total2
                    errors += errors2
    return total, errors
    
OK = "OK"
SKIP = "SKIP"
ERR = "ERR"

def test(eye_file) -> str:
    print(eye_file, '...', end='')
    stdout.flush()
    no_ext = eye_file.rsplit('.', maxsplit=1)[0]
    
    if os.path.exists(no_ext + '.out'):
        expected = no_ext + '.out'
    elif os.path.exists(no_ext + '.err'):
        expected = no_ext + '.err'
    else:
        t = 'No .out or .err file'
        padding = ' ' * max(0, 50 - len(eye_file) - len(t))
        print(f'{t}{padding}{CYAN}[SKIP]{R}')
        return SKIP

    with open(expected, encoding="utf8") as file:
        expected_output = file.read()

    input = None
    if os.path.exists(no_ext + '.in'):
        with open(no_ext + '.in', encoding="utf8") as file:
            input = memoryview(bytes(file.read(), 'utf-8'))

    out, stderr, exit_code = run(eye_file, input)
    
    padding = ' ' * max(0, 50 - len(eye_file))
    
    out = out.replace('\r\n', '\n')
    expected_output = expected_output.replace('\r\n', '\n')

    if expected_output == out:
        print(f'{padding}{GREEN}[OK]{R}')
        return OK
    else:
        print(f'{padding}{RED}[ERR]{R}\nFailed with status: {exit_code}, Output:')
        print(f'--------------------\n{out}\n--------------------')
        if exit_code != 0:
            print(f'Stderr:\n--------------------\n{stderr}\n--------------------')
        else:
            print('... but expected:')
            print(f'--------------------\n{expected_output}\n--------------------')
            print(f'Diff:\n--------------------\n{stderr}\n--------------------')
            f = open("eyebuild/out.eye", "w")
            f.write(out)
            f.close()
            f = open("eyebuild/expected.eye", "w")
            f.write(expected_output)
            f.close()
            subprocess.call(["diff", "eyebuild/out.eye", "eyebuild/expected.eye"])
        return ERR


def run(eye_file, input = ''):
    res = subprocess.run(run_cmd + [eye_file], capture_output=True, input=input)
    return res.stdout.decode('utf-8', 'replace'), res.stderr.decode('utf-8', 'replace'), res.returncode

def check_std():
    text = 'std library ...'
    pad = ' ' * (54 - len(text))
    print(f'{text}{pad}', end='')

    res = subprocess.run([eye_path, 'check', 'std', '--lib'], capture_output=True)
    if res.returncode == 0:
        print(f'{GREEN}[OK]{R}')
        return True
    else:
        print(f'{RED}[ERR]{R}')
        print(res.stdout.decode('utf-8', 'replace'))
        return False
    
if __name__ == '__main__':
    main()
