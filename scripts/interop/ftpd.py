"""A throw-away FTP server on 127.0.0.1:21 for testing hybrid fetches.

    python ftpd.py ROOT_DIR [LOGIN PASSWORD]

Serves files below ROOT_DIR read-only (USER/PASS/TYPE/CWD/PASV/RETR/QUIT) and
prints each command. Stop it with Ctrl+C or by killing its process id.
"""
import os
import socket
import sys
import threading


def serve(conn, root, login, password):
    f = conn.makefile("rwb")

    def say(t):
        f.write((t + "\r\n").encode())
        f.flush()

    say("220 test server")
    cwd, data = root, None
    while True:
        line = f.readline().decode("latin-1").rstrip("\r\n")
        if not line:
            break
        cmd, _, arg = line.partition(" ")
        print(cmd, "***" if cmd == "PASS" else arg, flush=True)
        if cmd == "USER":
            say("331 password" if arg == login else "530 no")
        elif cmd == "PASS":
            say("230 in" if arg == password else "530 no")
        elif cmd == "TYPE":
            say("200 ok")
        elif cmd == "CWD":
            new = os.path.normpath(os.path.join(cwd, arg))
            if new.startswith(root) and os.path.isdir(new):
                cwd = new
                say("250 ok")
            else:
                say("550 no such directory")
        elif cmd == "PASV":
            data = socket.socket()
            data.bind(("127.0.0.1", 0))
            data.listen(1)
            p = data.getsockname()[1]
            say("227 passive (127,0,0,1,%d,%d)" % (p // 256, p % 256))
        elif cmd == "RETR":
            path = os.path.join(cwd, os.path.basename(arg))
            if data is None or not os.path.isfile(path):
                say("550 no such file")
                continue
            say("150 sending")
            d, _ = data.accept()
            with open(path, "rb") as src:
                d.sendall(src.read())
            d.close()
            data.close()
            data = None
            say("226 done")
        elif cmd == "QUIT":
            say("221 bye")
            break
        else:
            say("502 not here")
    conn.close()


def main():
    root = os.path.abspath(sys.argv[1])
    login = sys.argv[2] if len(sys.argv) > 2 else "sstv"
    password = sys.argv[3] if len(sys.argv) > 3 else "secret"
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", 21))
    s.listen(4)
    print("listening on 127.0.0.1:21, pid", os.getpid(), flush=True)
    while True:
        conn, _ = s.accept()
        threading.Thread(target=serve, args=(conn, root, login, password), daemon=True).start()


if __name__ == "__main__":
    main()
