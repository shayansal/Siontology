"""Writes the X.509 test chains for AP2's x5c roots: test keys only, made
for crates/sion-cli/tests/ap2_vectors.rs and interop.rs, and nothing else.

    python3 make.py <dir>     (needs the `cryptography` package)

Each run makes new keys, so the files differ from run to run; the tests
read whichever were written.
"""
import datetime, sys, pathlib
from cryptography import x509
from cryptography.x509.oid import NameOID
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, rsa

out = pathlib.Path(sys.argv[1])
START = datetime.datetime(2025, 1, 1, tzinfo=datetime.timezone.utc)
END = datetime.datetime(2125, 1, 1, tzinfo=datetime.timezone.utc)

def name(cn):
    return x509.Name([x509.NameAttribute(NameOID.ORGANIZATION_NAME, "Siontology test"), x509.NameAttribute(NameOID.COMMON_NAME, cn)])

def cert(subject, key, issuer, issuer_key, alg, ca, path_len=None, usage="ee", start=START, end=END):
    b = (x509.CertificateBuilder().subject_name(name(subject)).issuer_name(name(issuer))
         .public_key(key.public_key()).serial_number(x509.random_serial_number())
         .not_valid_before(start).not_valid_after(end)
         .add_extension(x509.BasicConstraints(ca=ca, path_length=path_len), critical=True))
    if usage == "ca":
        b = b.add_extension(x509.KeyUsage(False, False, False, False, False, True, True, False, False), critical=True)
    elif usage == "ee":
        b = b.add_extension(x509.KeyUsage(True, False, False, False, False, False, False, False, False), critical=True)
    return b.sign(issuer_key, alg)

def write(n, c):
    (out / f"{n}.der").write_bytes(c.public_bytes(serialization.Encoding.DER))

def key_out(n, k):
    (out / f"{n}.p8").write_bytes(k.private_bytes(serialization.Encoding.DER, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))

root_k = ec.generate_private_key(ec.SECP384R1())
root = cert("Test root", root_k, "Test root", root_k, hashes.SHA384(), True, 1, "ca")
inter_k = ec.generate_private_key(ec.SECP256R1())
inter = cert("Test provider CA", inter_k, "Test root", root_k, hashes.SHA384(), True, 0, "ca")
leaf_k = ec.generate_private_key(ec.SECP256R1())
leaf = cert("Test provider signer", leaf_k, "Test provider CA", inter_k, hashes.SHA256(), False)
write("root", root); write("inter", inter); write("leaf", leaf); key_out("leaf", leaf_k)

rogue_k = ec.generate_private_key(ec.SECP256R1())
write("rogue-root", cert("Rogue root", rogue_k, "Rogue root", rogue_k, hashes.SHA256(), True, 1, "ca"))

noca_k = ec.generate_private_key(ec.SECP256R1())
write("noca-inter", cert("Not a CA", noca_k, "Test root", root_k, hashes.SHA384(), False, None, "ee"))
noca_leaf_k = ec.generate_private_key(ec.SECP256R1())
write("noca-leaf", cert("Under not a CA", noca_leaf_k, "Not a CA", noca_k, hashes.SHA256(), False)); key_out("noca-leaf", noca_leaf_k)

old_k = ec.generate_private_key(ec.SECP256R1())
write("expired-leaf", cert("Expired signer", old_k, "Test provider CA", inter_k, hashes.SHA256(), False,
    start=datetime.datetime(2020, 1, 1, tzinfo=datetime.timezone.utc), end=datetime.datetime(2021, 1, 1, tzinfo=datetime.timezone.utc)))
key_out("expired-leaf", old_k)

rsa_k = rsa.generate_private_key(public_exponent=65537, key_size=2048)
write("rsa-root", cert("RSA test root", rsa_k, "RSA test root", rsa_k, hashes.SHA256(), True, 0, "ca"))
rsa_leaf_k = ec.generate_private_key(ec.SECP256R1())
write("rsa-leaf", cert("Signer under RSA", rsa_leaf_k, "RSA test root", rsa_k, hashes.SHA256(), False)); key_out("rsa-leaf", rsa_leaf_k)
print(sorted(p.name for p in out.iterdir()))
