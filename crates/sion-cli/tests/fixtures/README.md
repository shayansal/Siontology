# Test fixtures

`rsa-2048.pem` is an RSA key made for the tests (`openssl genpkey
-algorithm RSA -pkeyopt rsa_keygen_bits:2048`), and `rsa-2048.n` its
modulus in base64url, as a JSON Web Key gives it. The tests sign with it
as Microsoft's Bot Framework, Google Chat and a Google service account
would. It is public, and signs nothing else.
