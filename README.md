## how to build
if you want to build it from source, you need rust (super simple install)
```
https://www.rust-lang.org/tools/install
```
then just
```
sudo ./target/debug/kube-forwarder --kube-config /Users/kubeconfig.yaml
```
to forward traffic to appropriate POD. Sudo is required because of http is running on port 80.
When forwarder is running, you can curl using kube-dns entries (curl -X GET http://your-app.namespace)

To make kube-forwarder working, you need to add necessary entries in /etc/hosts. To handle following request:
```
curl http://test-app1.namespace1/some/resource
```
add 
```
127.0.0.1 test-app1.namespace1
```
in /etc/hosts

## mocks
during local developemt you may want to return mocked response instead requesting port-forwarder app, to do this you can 
provide file with mocks
```
--mock-location mocks.json
```
where json file has following format 

```json
[
    {
        "host": "application.test",
        "method": "GET",
        "match_uri_regex": "/api/something",
        "mocked_response": {
            "squadName": "Super hero squad",
            "homeTown": "Metro City",
            "formed": 2016,
            "secretBase": "Super tower",
            "active": true,
            "members": [
              {
                "name": "Molecule Man",
                "age": 29,
                "secretIdentity": "Dan Jukes",
                "powers": ["Radiation resistance", "Turning tiny", "Radiation blast"]
              },
              {
                "name": "Madame Uppercut",
                "age": 39,
                "secretIdentity": "Jane Wilson",
                "powers": [
                  "Million tonne punch",
                  "Damage resistance",
                  "Superhuman reflexes"
                ]
              },
              {
                "name": "Eternal Flame",
                "age": 1000000,
                "secretIdentity": "Unknown",
                "powers": [
                  "Immortality",
                  "Heat Immunity",
                  "Inferno",
                  "Teleportation",
                  "Interdimensional travel"
                ]
              }
            ]
          }
          ,
        "headers" : {
            "content-type": "application/json",
            "location": "http://application.test/resource/203f972e-0496-4955-a481-a358be1004a2"
        },
        "status": 200
    }
]
```
then fowarder will match following request 
```
curl "http://application.test/api/something" | jq
{
  "squadName": "Super hero squad",
  "secretBase": "Super tower",
  "homeTown": "Metro City",
  "formed": 2016,
  "active": true,
  "members": [
    {
      "name": "Molecule Man",
      "age": 29,
      "secretIdentity": "Dan Jukes",
      "powers": [
        "Radiation resistance",
        "Turning tiny",
        "Radiation blast"
      ]
    },
    {
      "name": "Madame Uppercut",
      "age": 39,
      "secretIdentity": "Jane Wilson",
      "powers": [
        "Million tonne punch",
        "Damage resistance",
        "Superhuman reflexes"
      ]
    },
    {
      "name": "Eternal Flame",
      "age": 1000000,
      "secretIdentity": "Unknown",
      "powers": [
        "Immortality",
        "Heat Immunity",
        "Inferno",
        "Teleportation",
        "Interdimensional travel"
      ]
    }
  ]
}
```
and respond with mocked body.

## how it works
basically that is how it works
![howitworks](howitworks.png)
