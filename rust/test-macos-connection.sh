#!/usr/bin/env bash
# Connection test of the macOS build, run by the release workflow on a
# GitHub Mac after build-release-macos.sh:
#   - the one-time authorization (its root side, through sudo, as the
#     administrator password window would run it);
#   - a local OpenVPN test server that checks username + password + TOTP;
#   - the app connecting by itself (account set to connect on open);
#   - the helper refusing a .ovpn that would run a script as root;
#   - nothing left running after the app ends abruptly.
# Everything is fake test data. Screenshot and logs go to target/macos-test.
set -euo pipefail
cd "$(dirname "$0")"

app=target/macos-dist/VPN.app
exe=$(pwd)/$app/Contents/MacOS/VPN
saida=$(pwd)/target/macos-test
helper=/Library/PrivilegedHelperTools/io.github.antxj.vpn.helper
openvpn=/Library/PrivilegedHelperTools/io.github.antxj.vpn.openvpn
dns=/Library/PrivilegedHelperTools/io.github.antxj.vpn.dns-updown
seed=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ
pasta=$HOME/vpn-teste
dados="$HOME/Library/Application Support/VPN"
rm -rf "$saida" "$pasta" "$dados"
mkdir -p "$saida" "$pasta"

falhou() {
    echo "FAILED: $*" >&2
    cp "$dados"/logs/*.log "$saida/" 2>/dev/null || true
    exit 1
}

echo "--- one-time authorization (root side)"
sudo "$exe" --authorize-openvpn-user "$(id -un)" || falhou "authorization (exit $?)"
for f in $helper $openvpn $dns; do
    [ "$(stat -f '%Su %Lp' "$f")" = "root 755" ] || falhou "$f is not root-owned 755"
done
[ "$(stat -f '%Su %Lp' /etc/sudoers.d/vpn-openvpn)" = "root 440" ] || falhou "sudo rule"
sudo cat /etc/sudoers.d/vpn-openvpn
sudo visudo -c -q || falhou "visudo"
[ "$("$helper" --openvpn-helper-version)" = 1 ] || falhou "helper version"
"$openvpn" --version | head -1

echo "--- test PKI and server"
cd "$pasta"
openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.crt -days 2 \
    -subj /CN=vpn-teste-ca 2>/dev/null
for nome in servidor cliente; do
    openssl req -newkey rsa:2048 -nodes -keyout $nome.key -out $nome.csr -subj /CN=$nome 2>/dev/null
    openssl x509 -req -in $nome.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
        -out $nome.crt -days 2 2>/dev/null
done
"$openvpn" --genkey tls-crypt tc.key
cat > checa.py <<EOF
#!/usr/bin/python3
# username/password file from OpenVPN: line 1 user, line 2 password + TOTP
import base64, hashlib, hmac, struct, sys, time
def totp(t):
    h = hmac.new(base64.b32decode("$seed"), struct.pack(">Q", int(t) // 30), hashlib.sha1).digest()
    o = h[-1] & 15
    return "%06d" % ((struct.unpack(">I", h[o:o + 4])[0] & 0x7fffffff) % 1000000)
usuario, senha = open(sys.argv[1]).read().split("\n")[:2]
agora = time.time()
ok = usuario == "maria" and any(senha == "senha" + totp(agora + d) for d in (-30, 0, 30))
sys.exit(0 if ok else 1)
EOF
chmod 755 checa.py
cat > servidor.conf <<EOF
port 1194
proto udp4
local 127.0.0.1
dev tun
ca $pasta/ca.crt
cert $pasta/servidor.crt
key $pasta/servidor.key
dh none
tls-crypt $pasta/tc.key
server 10.99.0.0 255.255.255.0
topology subnet
keepalive 5 30
script-security 2
auth-user-pass-verify $pasta/checa.py via-file
verb 3
EOF
sudo "$openvpn" --config servidor.conf --daemon --log "$saida/servidor.log" --writepid "$saida/servidor.pid"
for _ in $(seq 20); do
    grep -q "Initialization Sequence Completed" "$saida/servidor.log" 2>/dev/null && break
    sleep 1
done
grep -q "Initialization Sequence Completed" "$saida/servidor.log" || falhou "test server"

inline() {
    echo "<ca>"; cat ca.crt; echo "</ca>"
    echo "<cert>"; cat cliente.crt; echo "</cert>"
    echo "<key>"; cat cliente.key; echo "</key>"
    echo "<tls-crypt>"; cat tc.key; echo "</tls-crypt>"
}
{ printf "client\ndev tun\nproto udp4\nremote 127.0.0.1 1194\nnobind\nauth-user-pass\nverb 3\n"; inline; } > teste.ovpn
{ printf "client\ndev tun\nproto udp4\nremote 127.0.0.1 1194\nnobind\nscript-security 2\nup /tmp/x.sh\n"; inline; } > perigoso.ovpn

echo "--- the helper refuses a .ovpn that runs a script as root"
recusa=$(sudo -n "$helper" --openvpn-helper "$pasta" --config "$pasta/perigoso.ovpn" </dev/null 2>&1 || true)
echo "$recusa"
echo "$recusa" | grep -q "directive not allowed in the .ovpn: script-security" || falhou "dangerous .ovpn"

echo "--- the app connects by itself"
cat > contas.json <<EOF
{"versao": 2, "idioma": "pt", "contas": [{"id": "c1", "nome": "Trabalho",
 "config": "$pasta/teste.ovpn", "usuario": "maria", "autenticacao": "SenhaMaisToken",
 "seed": "$seed", "senha": "senha", "conectar_ao_abrir": true}]}
EOF
# accounts written with the app's own encryption (key in a file: CI has no
# unlocked Keychain to use)
(cd "$OLDPWD" && VPN_SEM_CHAVEIRO=1 VPN_EXEMPLO_JSON="$pasta/contas.json" \
    cargo test --locked -q -- --ignored grava_contas_de_exemplo >/dev/null)
VPN_SEM_CHAVEIRO=1 VPN_SKIP_HINT=1 "$exe" >"$saida/app.log" 2>&1 &
pid_app=$!
conectado=""
for _ in $(seq 45); do
    if grep -q "Initialization Sequence Completed" "$dados/logs/openvpn-c1.log" 2>/dev/null; then
        conectado=1
        break
    fi
    sleep 1
done
sleep 3
screencapture -x "$saida/tela.png" || true
cp "$dados"/logs/*.log "$saida/" 2>/dev/null || true
[ -n "$conectado" ] || falhou "the app did not connect"
grep -E "Initialization Sequence Completed|PUSH_REPLY|net_route|dns" "$dados/logs/openvpn-c1.log" | head -5

echo "--- the app ends abruptly: the helper stops openvpn"
kill -9 $pid_app
sleep 8
if pgrep -fl "$openvpn --config /var/run/vpn-openvpn-helper"; then
    falhou "openvpn left behind"
fi
echo "nothing left behind"

sudo kill "$(cat "$saida/servidor.pid")" 2>/dev/null || true
echo "CONNECTION TEST OK"
