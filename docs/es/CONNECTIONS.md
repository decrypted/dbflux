# Conectar a una base de datos — Configuración avanzada

Esta guía cubre todo lo que hay en el Connection Manager más allá del formulario
básico de "host, port, user, password": llegar a una base de datos a través de
un túnel SSH, un proxy o AWS SSM; autenticarse con Auth Profiles gestionados por
un provider (AWS SSO); y obtener valores de campos individuales desde un secret
manager o parameter store en lugar de escribirlos directamente.

Para un primer recorrido (crear una conexión, explorar el schema, ejecutar
queries) consulta [Primeros pasos](GETTING_STARTED.md). Este documento empieza
por abrir el Connection Manager y elegir un driver, y luego cubre la pestaña
**Access** y los selectores de fuente de valores.

---

## Abrir el Connection Manager

Abre el Connection Manager para crear o editar conexiones:

- Pulsa `Ctrl+Shift+N` (`Cmd+Shift+N` en macOS).
- Desde el sidebar, pulsa `c`.
- O usa el command palette (`Ctrl+Shift+P` / `Cmd+Shift+P` en macOS) y ejecuta
  **Open connection manager**.

## Elegir un driver

El Connection Manager muestra un selector de drivers. Los drivers disponibles
dependen de los features con los que se compiló el binario; el build estándar
incluye SQLite, PostgreSQL, MySQL/MariaDB, Microsoft SQL Server, Amazon
Redshift, ClickHouse, TursoDB, DuckDB, MongoDB, DynamoDB, Redis, InfluxDB, CloudWatch
Logs, Amazon S3 y AWS Secrets. El selector agrupa los drivers por categoría:
Relacional, Documentos, Clave-valor, Series temporales y logs, Almacenamiento de
objetos, y Almacenes de secretos.
Los drivers RPC registrados externamente también aparecen aquí cuando están
configurados (ver `docs/RPC_SERVICES_CONFIG.md`).

Usa `/` para filtrar la lista de drivers, `j`/`k` (o las flechas) para moverte,
y `Enter` para seleccionar.

Cada driver ofrece su propio formulario de conexión. El formulario es dinámico:
solo muestra los campos que ese driver realmente necesita. Los drivers
respaldados por archivo, como SQLite, usan un formulario de ruta de archivo. La
mayoría de los drivers relacionales también aceptan una única cadena de
conexión; ver [Modo formulario vs. URI directa](#modo-formulario-vs-uri-directa).

La sección **Seguridad** cerca del final de la pestaña Main contiene el interruptor
**Solo lectura**. Cuando está activado, DBFlux bloquea los cambios hechos desde la
grilla de datos, el constructor visual de consultas y la comparación de esquemas en
esa conexión, rechaza las sentencias del editor de consultas que cambiarían datos o
esquema, y deniega las llamadas MCP por encima de la clase `read` (ver
[Conexiones de solo lectura](MCP_AI_INTEGRATION.md#conexiones-de-solo-lectura)).
Las sentencias de lectura se siguen ejecutando. En la app, activar el ajuste se aplica
de inmediato a una conexión abierta, mientras que desactivarlo surte efecto al reconectar.
MCP toma cualquiera de los dos cambios en su siguiente llamada. El editor clasifica cada sentencia por
su texto, así que una función con efectos secundarios llamada desde un `SELECT`, como
`setval` de PostgreSQL, se sigue ejecutando: las credenciales de solo lectura en la
base de datos siguen siendo el límite más fuerte.

La sección **Inicio**, la última de la pestaña Main, contiene el interruptor
**Conectar al iniciar**. Cuando está activado, DBFlux se conecta a esa conexión en
segundo plano cada vez que se inicia, igual que al hacer clic en la conexión: se
aplican los túneles SSH, los proxies, los hooks y los secretos del llavero, y una
conexión fallida muestra su error habitual. Una conexión cuya contraseña no está
guardada no se conecta al iniciar; en su lugar queda marcada como fallida, y
**Reintentar** pide la contraseña. Lo mismo ocurre con una conexión cuyo túnel SSH
necesita una frase de contraseña de la clave que no está guardada: **Reintentar**
pide la frase. Un solo aviso nombra todas las conexiones que no se conectaron
porque su contraseña no está guardada. El menú de la conexión en la barra lateral (clic
derecho, o el botón `…` de la fila) tiene el mismo interruptor, con una marca de
verificación mientras está activado.

Mientras **Reopen last connections** (Ajustes > General > Inicio y sesión) está
activado, ese ajuste decide por sí solo qué conexiones se abren al iniciar. El
interruptor se muestra entonces atenuado en ambos lugares, con una nota que nombra
ese ajuste, y no tiene efecto; conserva su valor, que vuelve a aplicarse cuando se
desactiva el ajuste.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/connections/driver-picker-dark.webp">
  <img src="../images/connections/driver-picker-light.webp" alt="El selector de drivers del Connection Manager, con los drivers agrupados en relacionales, de documentos, clave-valor, series temporales y almacenamiento de objetos">
</picture>

---

## La pestaña Access: cómo llega DBFlux a la base de datos

Cada conexión usa exactamente **un** método de acceso, elegido en el desplegable
**Access Method**. Cambiar de método borra la configuración de los demás — una
conexión es Direct, SSH, Proxy o SSM, nunca una combinación.

| Método                  | Qué hace                                                                                                                                                                                            |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Direct**              | Conecta directamente al host/port de la pestaña Main. Puede seguir resolviendo fuentes de valores por campo (ver [Fuentes de valores](#value-sources-secret-manager-parameter-store-auth-session)). |
| **SSH Tunnel**          | Abre un port-forward local a través de un host SSH, y conecta a través de él.                                                                                                                       |
| **Proxy**               | Enruta la conexión a través de un proxy SOCKS5 o HTTP/HTTPS.                                                                                                                                        |
| **SSM Port Forwarding** | Usa AWS Systems Manager para hacer port-forward a una instancia y conecta a través del túnel. Requiere el build feature `aws`.                                                                      |

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/connections/access-tab-dark.webp">
  <img src="../images/connections/access-tab-light.webp" alt="La pestaña Access de una conexión PostgreSQL con la lista Access Method abierta en Direct, SSH Tunnel, Proxy y SSM Port Forwarding">
</picture>

### Qué ocurre cuando pulsas Connect

DBFlux ejecuta un pipeline pre-connect fijo antes de que el driver abra un
socket:

1. **Authenticating** — valida o refresca la sesión del Auth Profile
   seleccionado (aquí puede ocurrir un login por navegador de AWS SSO).
2. **Resolving values** — resuelve cada fuente de valores por campo (secret
   manager, parameter store, variable de entorno, campo de auth-session) y las
   aplica a la configuración.
3. **Opening access** — establece el túnel SSH, el proxy o la sesión SSM (o
   nada, en el caso de Direct).
4. **Driver connect + schema fetch** — el driver conecta y DBFlux carga el
   schema superficial.

Los **hooks** de conexión (si tienes alguno vinculado) se ejecutan en las fases
PreConnect, PostConnect, PreDisconnect y PostDisconnect alrededor de este
pipeline. Ver [Settings & Hooks](SETTINGS.md#connection-hooks).

---

## Cuando una conexión falla

Si un intento de conexión falla, el error aparece en un toast y la fila de la
conexión conserva un ícono de error rojo. Pasa el puntero sobre el ícono para
leer el error. Elige **Reintentar** en el menú contextual de la fila, o presiona `Enter`
sobre la fila, para volver a conectar. La marca se borra cuando empieza un nuevo
intento, cuando la conexión tiene éxito o cuando editas la conexión.

## Desconectar con una consulta en ejecución

Desconectar una conexión que todavía tiene una consulta en ejecución pregunta
primero: **Cancelar consulta** detiene la consulta y mantiene la conexión
abierta, **Seguir esperando** no toca ninguna de las dos, y **Desconectar de
todos modos** cancela la consulta y desconecta. `Enter` elige **Cancelar
consulta** y `Escape` elige **Seguir esperando**. Cerrar la ventana de DBFlux
mientras hay una consulta en ejecución en cualquier conexión hace la misma
pregunta, nombrando las conexiones, con **Salir de todos modos** en lugar de
**Desconectar de todos modos**.

---

## Túneles SSH

Puedes usar un túnel SSH de dos formas:

- **Referenciar un túnel guardado** — elige un perfil de túnel que gestiones de
  forma centralizada en **Settings → SSH Tunnels**. Recomendado cuando
  reutilizas el mismo bastion en varias conexiones.
- **Inline** — rellena los campos SSH directamente en la pestaña Access. Más
  adelante puedes pulsar **Save as tunnel** para convertirlo en un perfil
  reutilizable.

### Campos SSH

| Campo                            | Notas                                                                                                              |
| -------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| **Host** / **Port**              | El servidor SSH. El puerto suele ser `22`.                                                                         |
| **Username**                     | Usuario SSH.                                                                                                       |
| **Auth method**                  | **Private Key** o **Password**.                                                                                    |
| **Key path** (Private Key)       | Ruta a la clave privada. **Déjalo vacío para usar tu SSH agent o las claves por defecto** (`~/.ssh/id_rsa`, etc.). |
| **Key passphrase** (Private Key) | Opcional; se guarda en el keyring del sistema operativo al marcar **Save**.                                        |
| **Password** (Password auth)     | Se guarda en el keyring del sistema operativo al marcar **Save**.                                                  |

No existe una opción separada de "SSH agent" — la autenticación basada en agente
es lo que obtienes al elegir **Private Key** y dejar la ruta de la clave vacía.

**Test SSH** verifica el túnel sin guardar la conexión.

### Referenciar un host de tu configuración SSH

El desplegable **SSH config host** lista cada `Host` que define tu
`~/.ssh/config`, con el usuario, el nombre de host y el puerto a los que
resuelve cada uno. Al elegir uno se guarda el alias y no los valores a los que
resuelve, de modo que una edición posterior de tu configuración SSH se aplica
en la siguiente conexión sin tocar el perfil. Mientras hay un alias
seleccionado, los campos **Host**, **Port** y **Username** no muestran nada y
quedan deshabilitados —el alias es el destino—, y una ruta de clave vacía toma
el `IdentityFile` que ese host indique, o el SSH agent y las claves por defecto
si no indica ninguna. `Include`, `Match` y los tokens `%h`, `%r` y `%d` dentro
de un `IdentityFile` se resuelven como los resuelve OpenSSH.

En Linux y macOS, un host con `ProxyCommand` funciona igual que en una
terminal: DBFlux ejecuta el comando a través de tu `$SHELL` y habla SSH por su
entrada y salida estándar, así que funcionan proxies de acceso como
`cloudflared access ssh --hostname %h`. Se expanden los tokens `%h`, `%p`,
`%r`, `%n` y `%%`; cualquier otro token hace fallar la conexión antes de que se
ejecute el comando. Lo mismo ocurre con un valor para `%h`, `%n` o `%r` que
contenga espacios o un carácter que la shell interpretaría (comillas, `$`,
comillas invertidas, `;`, `|` y similares) o que empiece por `-`, de modo que
un alias de host no puede inyectar comandos en la shell. La salida de error
del comando, donde un proxy de acceso imprime su URL de inicio de sesión, solo
se registra en el nivel debug, para que las URL de un solo uso y los tokens no
acaben en el log de auditoría; ejecuta DBFlux con `RUST_LOG=debug` para verla.
Si el handshake SSH a través del comando falla, el error muestra sus tres
últimas líneas, acortadas, con las URL reducidas a su host (y otras query strings
eliminadas) y las credenciales sustituidas por `***`. El log solo nombra el programa que ejecuta el comando; la
línea de comando completa, que puede llevar un token de acceso, solo se
registra en el nivel debug.

Un host cuya configuración necesite `ProxyJump`, o `ProxyCommand` en Windows,
aparece en la lista con una marca: DBFlux no puede alcanzarlo, y la conexión falla con ese
motivo en lugar de conectar directamente al bastión. Las claves de host se
siguen verificando contra el almacén propio de DBFlux en su directorio de
datos, no contra tu `~/.ssh/known_hosts`, así que un host al que llevas años
llegando desde una terminal igual te pide confirmar su clave la primera vez.

### Dónde viven los secretos SSH

Las passphrases y contraseñas se guardan en el **keyring del sistema
operativo**, nunca en la base de datos. La casilla **Save** solo aparece cuando
hay un keyring disponible; si no lo hay, los secretos no se persisten y tendrás
que reintroducirlos cada sesión. Ver [Datos y privacidad →
Secretos](PRIVACY.md#secretos-y-el-keyring-del-sistema-operativo).

---

## Proxies

Los proxies se gestionan en **Settings → Proxies**; la pestaña Access solo
*selecciona* un proxy guardado y muestra sus detalles. Si no tienes ninguno, la
pestaña te enlaza a Settings.

| Campo               | Notas                                                                                                                                                                                                              |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Type**            | `SOCKS5`, `HTTP` o `HTTPS`. Puerto por defecto: `1080` para SOCKS5, `8080` para HTTP/HTTPS.                                                                                                                        |
| **Host** / **Port** | El endpoint del proxy.                                                                                                                                                                                             |
| **Auth**            | `None`, o `Basic` con un usuario (la contraseña se guarda en el keyring).                                                                                                                                          |
| **No Proxy**        | Hosts/patrones separados por comas para omitir el proxy. Soporta `*` (todos), hosts exactos y coincidencias de sufijo (con o sin punto inicial), sin distinguir mayúsculas/minúsculas. **No soporta rangos CIDR.** |
| **Enabled**         | Cuando un perfil de proxy está deshabilitado, la conexión recurre a una conexión **directa** (con un aviso) en lugar de fallar.                                                                                    |

> **Aviso:** un proxy deshabilitado, o un host remoto que coincide con **No
> Proxy**, resulta en una conexión directa silenciosa. Si esperabas que el
> tráfico pasara por el proxy y no fue así, revisa primero estos dos casos.

---

## Auth Profiles (AWS SSO y credenciales compartidas)

Los Auth Profiles contienen autenticación gestionada por un provider que DBFlux
resuelve en el momento de conectar. Se crean en **Settings → Auth Profiles** y
se seleccionan por conexión. En este build los providers integrados son **solo
AWS**:

| Provider                   | Para qué se usa                                                                                          |
| -------------------------- | -------------------------------------------------------------------------------------------------------- |
| **AWS SSO**                | Login de IAM Identity Center (SSO) que resuelve una cuenta + rol.                                        |
| **AWS SSO Session**        | Una sesión SSO reutilizable (Start URL + región + scopes) de la que pueden heredar los perfiles AWS SSO. |
| **AWS Shared Credentials** | Un perfil con nombre escrito en `~/.aws/credentials` (access key / secret / session token opcional).     |

Los providers de auth RPC registrados externamente pueden añadir más entradas
aquí; ver [RPC Services](RPC_SERVICES_CONFIG.md).

> Los perfiles AWS que DBFlux refleja en vivo desde tu `~/.aws/config` aparecen
> como **de solo lectura** — puedes seleccionarlos pero no editarlos aquí; edita
> los archivos de AWS directamente.

### Crear un perfil AWS SSO

El formulario está guiado por el provider. Para AWS SSO rellenas:

| Campo             | Notas                                                                                                                                                 |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Profile name**  | p. ej. `dev`.                                                                                                                                         |
| **SSO session**   | Referencia opcional a un perfil **AWS SSO Session**. Al fijarla, el Start URL y la región se heredan y sus campos inline se deshabilitan visualmente. |
| **SSO Start URL** | La URL de tu portal de Identity Center (omítela si usas una sesión).                                                                                  |
| **Region**        | p. ej. `us-east-1`.                                                                                                                                   |
| **Endpoint URL**  | Opcional, p. ej. `http://localhost:4566` para LocalStack. Solo afecta a Secrets Manager y SSM, mediante una sección `[services]` en `~/.aws/config`. Los perfiles AWS Shared Credentials tienen el mismo campo. |
| **Account**       | Un desplegable que se rellena **después de iniciar sesión** — lista las cuentas a las que tu sesión SSO tiene acceso.                                 |
| **Role**          | Un desplegable que se rellena una vez elegida una cuenta.                                                                                             |

Los desplegables **Account** y **Role** son dinámicos: requieren una sesión SSO
activa y se refrescan cuando cambian sus dependencias. Si están vacíos, inicia
sesión primero (ver abajo).

El **SSO Wizard** ofrece el mismo flujo como un creador guiado, paso a paso:
introduce nombre, Start URL y región; te inicia sesión y luego lista las cuentas
y roles para que elijas.

### El flujo de login SSO

El access token que guarda `aws sso login` dura alrededor de una hora, mientras
que el refresh token guardado con él para un perfil que usa `sso_session` dura
mucho más. Cuando el access token ha expirado, DBFlux primero lo renueva con ese
refresh token, como hacen el AWS CLI y los SDK, y lo escribe de vuelta en el
mismo archivo de `~/.aws/sso/cache` (se conservan las demás entradas y el
archivo sigue siendo legible solo por ti), así que no se abre el navegador. Solo hace falta un login
cuando no hay sesión en caché, el refresh token o el registro del cliente ha
expirado o fue revocado, o la renovación falla; y cada vez que expira el access
token de un perfil que define `sso_start_url` sin `sso_session`, porque esos
logins no guardan refresh token.

Cuando una conexión (o los desplegables Account/Role) necesita un login SSO,
DBFlux abre un modal de login:

- **Abre tu navegador automáticamente** en la URL de verificación.
- Si el navegador no se puede abrir, el modal muestra la URL con una acción
  **Copy URL** para que la abras manualmente.
- DBFlux continúa automáticamente en cuanto terminas de autenticarte en el
  navegador.
- **El login SSO expira a los 5 minutos.**

### Seleccionar un Auth Profile por conexión

- **Modos Direct, SSH Tunnel y Proxy** — el Auth Profile es *opcional* y
  aparece debajo de los campos propios del método. Se usa solo para resolver
  fuentes de valores Secret/Parameter/Auth (siguiente sección); se resuelven
  antes de abrir el túnel o el proxy, así que un túnel SSH a un bastión puede
  tomar la contraseña de la base de datos de Secret Manager.
- **Modo SSM** — el Auth Profile es **obligatorio**.
- Cambiar el método de acceso conserva el Auth Profile seleccionado.
- Si algún campo usa una fuente de valores Secret/Parameter cuyo provider es un
  auth provider, se requiere un Auth Profile correspondiente o la conexión se
  rechaza antes de conectar.

Cada fila de perfil tiene botones **Manage**, **Login** y **Refresh**. **Login**
solo está habilitado cuando el perfil seleccionado realmente necesita iniciar
sesión.

---

## SSM Port Forwarding (acceso gestionado)

El acceso "gestionado" permite que un provider abra el camino hacia el host por
ti. La implementación incluida es **AWS SSM Port Forwarding** (requiere el
feature `aws`).

| Campo            | Notas                                                               |
| ---------------- | ------------------------------------------------------------------- |
| **Instance ID**  | Instancia EC2 de destino. Soporta un selector de fuente de valores. |
| **Region**       | Por defecto `us-east-1` si se deja en blanco.                       |
| **Remote Port**  | El puerto de la instancia al que se hace el forward.                |
| **Auth Profile** | **Obligatorio** — el perfil AWS usado para iniciar la sesión SSM.   |

El puerto **local** del túnel lo asigna automáticamente DBFlux y el sistema
operativo — solo el puerto remoto es configurable.

SSM necesita el AWS CLI y el plugin de Session Manager en el `PATH`. En macOS y
Linux, cuando DBFlux se abre fuera de una terminal (desde Finder, el Dock o una
entrada de escritorio), lee el `PATH` de tu shell de login al arrancar, así que
encuentra las herramientas instaladas con Homebrew o agregadas en la
configuración del shell. Si el shell tarda más de cinco segundos en iniciar,
DBFlux conserva el `PATH` con el que se abrió y registra una advertencia.

---

## Fuentes de valores: Secret Manager, Parameter Store, Auth Session

Cualquier campo individual de una conexión (host, password, etc.) puede obtener
su valor desde una fuente externa en lugar de un literal. Haz clic en el
selector de fuente junto a un campo y elige:

| Fuente                   | Qué hace                                                           |
| ------------------------ | ------------------------------------------------------------------ |
| **Literal**              | El valor que escribes (por defecto).                               |
| **Environment Variable** | Se lee de una variable de entorno por nombre.                      |
| **Dotenv File**          | Se lee una clave de un archivo `.env` tuyo, escrito como `ruta#CLAVE`. |
| **Secret Manager**       | Se obtiene de un secret provider (AWS Secrets Manager).            |
| **Parameter Store**      | Se obtiene de un parameter provider (AWS SSM Parameter Store).     |
| **Auth Session Field**   | Toma un campo de la sesión/credenciales resuelta del Auth Profile. |

Notas:

- Las fuentes Secret/Parameter pueden apuntar a una **clave JSON** dentro de un
  secreto JSON, de forma que un único documento JSON almacenado puede alimentar
  varios campos. Escribe la clave después del nombre o ARN del secreto como
  `<locator>#<clave>`, por ejemplo `rds!db-1a2b#password` para la contraseña de
  un secreto que gestiona RDS. La indicación bajo el campo dice si se usa el valor
  completo o una sola clave. Un secreto de Secrets Manager que contiene un objeto
  JSON pero se referencia sin clave se rechaza al conectar, y el error lista las
  claves del secreto (nunca sus valores).
- Los valores resueltos se cachean durante **5 minutos** para evitar volver a
  obtenerlos en cada reconexión.
- Las fuentes Secret/Parameter respaldadas por un auth provider **requieren un
  Auth Profile** en la conexión (se valida antes de conectar).
- Para llegar a LocalStack o a un endpoint de VPC, define **Endpoint URL** en el
  Auth Profile de AWS. El SDK de AWS también respeta
  `AWS_ENDPOINT_URL_SECRETS_MANAGER` y `AWS_ENDPOINT_URL_SSM`.

> **Atención:** DBFlux escribe el endpoint en una sección `[services]` a la que
> apunta el perfil, por lo que solo se aplica a Secrets Manager y SSM. Eso
> incluye los comandos `secretsmanager` y `ssm` del AWS CLI y el port
> forwarding de SSM que usen ese perfil. DynamoDB, S3 y CloudWatch mantienen sus
> endpoints por defecto. Una sección `[services]` compartida con otro perfil
> nunca se edita: DBFlux le da a este perfil su propia copia.

### Variables de entorno y archivos dotenv

Ambas fuentes se leen cada vez que DBFlux se conecta, así que un valor rotado se
usa en la siguiente conexión sin reiniciar DBFlux. DBFlux guarda solo la
referencia (el nombre de la variable, o la ruta del archivo y la clave), nunca
el valor, y ninguna de las dos fuentes se cachea.

| Entrada del campo | Fuente | Lee |
|-------------------|--------|-----|
| `DB_PASSWORD` | Environment Variable | `DB_PASSWORD` del entorno con el que se inició DBFlux |
| `~/.config/dbflux/prod.env#DB_PASSWORD` | Dotenv File | `DB_PASSWORD` de ese archivo |

Un archivo dotenv debe cumplir estas reglas; si no, la conexión falla con un
error que nombra el archivo y la clave pero nunca muestra un valor:

- La ruta es absoluta o empieza con `~/`.
- Es un archivo regular (se sigue un symlink a uno) de hasta 64 KiB y 1024
  entradas, codificado en UTF-8.
- En Linux y macOS, solo su dueño puede leerlo o escribirlo (`chmod 600`).
- Cada línea está vacía, es un comentario `#` o es `CLAVE=valor`, con un prefijo
  `export` opcional. Un valor puede ir sin comillas (se quita un ` # comentario`
  final), entre comillas simples (se toma tal cual) o entre comillas dobles (se
  interpretan `\n`, `\t`, `\"`, `\\` y `\$`). Los valores ocupan una línea y `${VAR}` no se expande.
  Si una clave aparece dos veces, gana la última definición.

| Problema | Error |
|----------|-------|
| Variable no definida | `Environment variable 'DB_PASSWORD' not set` |
| Variable definida como cadena vacía | `Environment variable 'DB_PASSWORD' is empty` |
| Archivo inexistente | `Dotenv file '~/prod.env' does not exist` |
| Archivo sin permiso de lectura | `Dotenv file '~/prod.env' cannot be read: permission denied` |
| Archivo abierto a otros usuarios | `Dotenv file '~/prod.env' can be read or changed by other users (mode 644); restrict it with chmod 600` |
| Línea que no es una entrada | `Dotenv file '~/prod.env' has a malformed entry on line 4` |
| Clave ausente en el archivo | `Key 'DB_PASSWORD' is not defined in dotenv file '~/prod.env'` |
| Clave con valor vacío | `Key 'DB_PASSWORD' in dotenv file '~/prod.env' is empty` |

Una lectura fallida detiene la conexión. DBFlux nunca recurre a una contraseña
guardada en el keyring para ese campo.

> **Atención:** un lanzador de escritorio inicia DBFlux con el entorno de la
> sesión, no con el de tu shell. Una variable exportada solo en `~/.zshrc` o
> `~/.bashrc` no es visible para DBFlux abierto desde el menú de aplicaciones;
> usa un archivo dotenv, o define la variable en el entorno de tu sesión.

---

## Modo formulario vs. URI directa

La mayoría de los drivers relacionales te permiten indicar los detalles de
conexión como campos individuales o como una única cadena de conexión. Un
interruptor **Use URI** en la pestaña Main alterna entre ambos.

- El modo URI está disponible para **PostgreSQL, MySQL/MariaDB, SQL Server,
  MongoDB y Redis**. SQLite, DynamoDB, CloudWatch e InfluxDB usan sus propios
  formularios basados en campos.
- Con el modo URI **activado**, el campo único de URI es la fuente de verdad y
  los campos individuales se ignoran; con él **desactivado**, se usan los
  campos.
- Una contraseña embebida en una URI se extrae y se guarda por separado (en el
  keyring al guardarla), no se conserva en el texto de la URI.
- Al conectar a través de un túnel SSH/proxy/SSM, DBFlux siempre usa la
  configuración basada en campos (reescrita a `127.0.0.1:<local port>`), incluso
  si escribiste una URI. Las opciones de consulta de la URI no se usan entonces;
  defínelas como parámetros del controlador (ver Parámetros del controlador), que
  se aplican en todos los modos.

---

## Parámetros del controlador

La sección **Driver parameters** de la pestaña Main, debajo de Transport, guarda
opciones adicionales `clave = valor` para el controlador, como la pestaña
Advanced de otros clientes de bases de datos. Aparece en los controladores que
aceptan parámetros: **MySQL/MariaDB** y **PostgreSQL**. Los demás no la muestran.

- Añade una fila con los campos de clave y valor y **Add** (`Enter` sobre él);
  **Edit** de una fila la carga de nuevo en los campos, donde **Update** la
  reescribe en su sitio, y **Remove** la quita. Una clave escrita en los campos
  pero aún no añadida también se guarda con la conexión.
- Los parámetros se aplican en **todos los modos de conexión**: modo
  formulario, modo URI y conexiones a través de un túnel SSH, un proxy o SSM. Se
  guardan fuera del host y el puerto, así que la reescritura del túnel a
  `127.0.0.1:<local port>` los conserva.
- **Precedencia:** una fila posterior gana a una anterior con la misma clave. En
  modo URI se aplican primero las opciones de consulta de la propia URI y los
  parámetros las reemplazan.
- Las claves que pertenecen al formulario se rechazan con un error que nombra
  la clave: host, puerto, usuario, contraseña, base de datos, cualquier ajuste
  SSL/TLS y, en MySQL, también `socket` y `prefer_socket`, que saltarían el
  host, el túnel o el proxy configurados. Una clave desconocida o un valor que
  el controlador no entiende se rechaza igual, al añadir la fila y de nuevo al
  guardar, probar y conectar.
- Los valores son **texto plano**. No son fuentes de valores, no se guardan en
  el keyring y viajan sin cifrar en los paquetes de exportación. No pongas
  contraseñas ni tokens en ellos.

| Controlador | Claves admitidas |
|--------|----------------|
| MySQL / MariaDB | `compress` (`true`, `false`, `fast`, `best`, `0`-`9`), `tcp_connect_timeout_ms`, `tcp_keepalive_time_ms`, `tcp_keepalive_probe_interval_secs` / `tcp_keepalive_probe_count` (Linux, macOS), `tcp_user_timeout_ms` (Linux), `stmt_cache_size`, `max_allowed_packet`, `secure_auth`, `enable_cleartext_plugin` |
| PostgreSQL | `application_name`, `options` (por ejemplo `-c statement_timeout=5000`), `connect_timeout`, `tcp_user_timeout`, `keepalives`, `keepalives_idle`, `keepalives_interval`, `keepalives_retries`, `target_session_attrs` (`any`, `read-write`, `read-only`), `channel_binding` (`disable`, `prefer`, `require`) |

Las claves de MySQL siguen las opciones de URL del cliente Rust `mysql` y él las
valida, así que se aplican sus reglas (por ejemplo, acepta un nivel en
`compress`, pero siempre comprime con su nivel por defecto). Las opciones de
pool `pool_min`, `pool_max`, `reset_connection` y `check_health` se rechazan:
DBFlux abre conexiones simples, no un pool. Las claves de PostgreSQL siguen los
parámetros de conexión de libpq; los tiempos de espera son segundos enteros.
DBFlux envía su propia identidad de cliente como `application_name`
(PostgreSQL) y `program_name` (MySQL); un parámetro `application_name`
explícito la reemplaza.

### Compresión en MySQL

En MySQL y MariaDB la sección Transport tiene un selector **Compression** con
**Auto**, **On** y **Off**. Edita el parámetro `compress` de la lista, así que el
selector y la lista siempre coinciden:

- **Auto** es la ausencia de una fila `compress`. Una conexión a través de un
  túnel SSH, un proxy o SSM se comprime; una conexión directa no.
- **On** escribe `compress=true`. Una fila con un nivel como `compress=best`
  también se muestra como On.
- **Off** escribe `compress=false`.

Auto es el valor por defecto, así que las conexiones MySQL existentes que pasan
por un túnel, un proxy o SSM empiezan a usar compresión del protocolo tras la
actualización. La compresión solo se activa cuando el servidor la ofrece, así
que un servidor sin soporte de compresión sigue conectando, sin comprimir.
Elige **Off** para mantener sin comprimir una conexión por túnel.

---

## Referencia rápida: problemas frecuentes

- **Un método de acceso por conexión.** Cambiar de método borra los demás.
- **Los secretos viven en el keyring del sistema operativo**, nunca en la base
  de datos de DBFlux. Si el keyring no está disponible, las casillas "Save"
  desaparecen y los secretos no se conservan.
- **Una contraseña sin guardar se pide al conectar.** Si la contraseña de una
  conexión no está guardada en el keyring ni proviene de una fuente de valor,
  DBFlux la pide antes de conectar, y cada base de datos que abre esa conexión
  la usa hasta que te desconectes. Déjala vacía para servidores que no piden
  contraseña. **Recordar hasta cerrar DBFlux** la guarda en memoria, nunca en
  el keyring, para que las siguientes conexiones no la vuelvan a pedir;
  Cancelar deja la conexión cerrada.
- **Un proxy deshabilitado o una coincidencia en No-Proxy conecta directamente
  de forma silenciosa.**
- **`No Proxy` no soporta CIDR** — lista hosts/sufijos, no rangos de IP.
- **SSM y las fuentes de valores respaldadas por auth requieren ambas un Auth
  Profile.**
- **Solo los providers de auth de AWS están integrados** (SSO, SSO Session,
  Shared Credentials). Otros providers vienen de servicios RPC externos.
- **El login SSO abre el navegador automáticamente y expira a los 5 minutos.**

## Relacionado

- [Primeros pasos](GETTING_STARTED.md) — el flujo básico de conectar/consultar/resultados.
- [Settings & Hooks](SETTINGS.md) — gestión de perfiles SSH/proxy/auth y hooks.
- [Datos y privacidad](PRIVACY.md#tus-datos-en-este-equipo) — dónde se almacenan las
  credenciales y los datos.
- [RPC Services](RPC_SERVICES_CONFIG.md) — drivers externos y auth providers.
